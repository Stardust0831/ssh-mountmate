use std::cmp::Reverse;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::paths::AppPaths;
use crate::storage::{FileLock, StorageError};

const HASH_PREFIX_LENGTH: usize = 16;
const EMBEDDED_RCLONE: &[u8] = include_bytes!(env!("SSH_MOUNTMATE_EMBEDDED_RCLONE_PATH"));
const EMBEDDED_RCLONE_SHA256: &str = env!("SSH_MOUNTMATE_EMBEDDED_RCLONE_SHA256");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcloneSource {
    Configured,
    Bundled,
    Embedded,
    Managed,
    SystemPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRclone {
    pub path: PathBuf,
    pub source: RcloneSource,
    pub sha256: Option<String>,
}

#[derive(Debug, Error)]
pub enum RcloneBinaryError {
    #[error("bundled rclone is missing its SHA-256 manifest: {0}")]
    MissingManifest(PathBuf),
    #[error("invalid rclone SHA-256 manifest: {0}")]
    InvalidManifest(PathBuf),
    #[error("rclone SHA-256 mismatch for {path}: expected {expected}, got {actual}")]
    DigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

pub fn binary_name(windows: bool) -> &'static str {
    if windows { "rclone.exe" } else { "rclone" }
}

pub fn resolve_rclone(
    paths: &AppPaths,
    app_root: &Path,
    configured: Option<&Path>,
) -> Result<Option<ResolvedRclone>, RcloneBinaryError> {
    let windows = cfg!(windows);
    if let Some(path) = configured.filter(|path| path.is_file()) {
        return Ok(Some(ResolvedRclone {
            path: path.to_owned(),
            source: RcloneSource::Configured,
            sha256: None,
        }));
    }

    for bundled in bundled_candidates(app_root, windows) {
        if bundled.is_file() {
            let digest = verify_bundled(&bundled)?;
            let path = materialize_bundled(paths, &bundled, &digest, windows)?;
            return Ok(Some(ResolvedRclone {
                path,
                source: RcloneSource::Bundled,
                sha256: Some(digest),
            }));
        }
    }

    if !EMBEDDED_RCLONE.is_empty() {
        let path = materialize_embedded(paths, windows)?;
        return Ok(Some(ResolvedRclone {
            path,
            source: RcloneSource::Embedded,
            sha256: Some(EMBEDDED_RCLONE_SHA256.into()),
        }));
    }

    let mut managed_dirs = vec![paths.managed_bin_dir()];
    managed_dirs.extend(paths.legacy_managed_bin_dirs());
    for directory in &managed_dirs {
        let candidate = directory.join(binary_name(windows));
        if candidate.is_file() {
            return Ok(Some(ResolvedRclone {
                path: candidate,
                source: RcloneSource::Managed,
                sha256: None,
            }));
        }
    }
    for directory in &managed_dirs {
        if let Some((path, digest)) = newest_valid_materialized(directory, windows)? {
            return Ok(Some(ResolvedRclone {
                path,
                source: RcloneSource::Managed,
                sha256: Some(digest),
            }));
        }
    }

    let executable = binary_name(windows);
    if let Some(path) = find_in_path(executable, env::var_os("PATH").as_deref()) {
        return Ok(Some(ResolvedRclone {
            path,
            source: RcloneSource::SystemPath,
            sha256: None,
        }));
    }
    for path in common_paths(executable) {
        if path.is_file() {
            return Ok(Some(ResolvedRclone {
                path,
                source: RcloneSource::SystemPath,
                sha256: None,
            }));
        }
    }
    Ok(None)
}

pub fn application_root(executable: &Path) -> PathBuf {
    let parent = executable.parent().unwrap_or_else(|| Path::new("."));
    if parent.file_name().is_some_and(|name| name == "MacOS")
        && parent
            .parent()
            .is_some_and(|contents| contents.file_name().is_some_and(|name| name == "Contents"))
    {
        parent.parent().unwrap().to_owned()
    } else {
        parent.to_owned()
    }
}

pub fn bundled_candidates(app_root: &Path, windows: bool) -> [PathBuf; 3] {
    let binary = binary_name(windows);
    [
        app_root.join("bin").join(binary),
        app_root.join("resources").join("bin").join(binary),
        app_root.join("Resources").join("bin").join(binary),
    ]
}

pub(crate) fn verify_bundled(path: &Path) -> Result<String, RcloneBinaryError> {
    let manifest = path.with_file_name(format!(
        "{}.sha256",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let content = fs::read_to_string(&manifest).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            RcloneBinaryError::MissingManifest(manifest.clone())
        } else {
            RcloneBinaryError::Io {
                path: manifest.clone(),
                source,
            }
        }
    })?;
    let expected = content
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| RcloneBinaryError::InvalidManifest(manifest.clone()))?;
    let actual = file_sha256(path)?;
    if actual != expected {
        return Err(RcloneBinaryError::DigestMismatch {
            path: path.to_owned(),
            expected,
            actual,
        });
    }
    Ok(actual)
}

fn materialize_bundled(
    paths: &AppPaths,
    source: &Path,
    digest: &str,
    windows: bool,
) -> Result<PathBuf, RcloneBinaryError> {
    materialize_verified(paths, digest, windows, |target| {
        copy_executable(source, target, windows)
    })
}

fn materialize_embedded(paths: &AppPaths, windows: bool) -> Result<PathBuf, RcloneBinaryError> {
    let digest = EMBEDDED_RCLONE_SHA256;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RcloneBinaryError::InvalidManifest(PathBuf::from(
            "embedded:rclone",
        )));
    }
    materialize_verified(paths, digest, windows, |target| {
        write_executable_bytes(target, EMBEDDED_RCLONE, windows)
    })
}

fn materialize_verified(
    paths: &AppPaths,
    digest: &str,
    windows: bool,
    write: impl FnOnce(&Path) -> Result<(), RcloneBinaryError>,
) -> Result<PathBuf, RcloneBinaryError> {
    let directory = paths.managed_bin_dir();
    let suffix = if windows { ".exe" } else { "" };
    let target = directory.join(format!(
        "rclone-{}{}",
        &digest[..HASH_PREFIX_LENGTH],
        suffix
    ));
    let _lock = FileLock::acquire(
        &directory.join(".rclone-materialize.lock"),
        Duration::from_secs(180),
    )?;
    if target.is_file() && file_sha256(&target)? == digest {
        return Ok(target);
    }
    if target.exists() {
        fs::remove_file(&target).map_err(|source| RcloneBinaryError::Io {
            path: target.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&directory).map_err(|source| RcloneBinaryError::Io {
        path: directory.clone(),
        source,
    })?;
    let temporary = directory.join(format!(".rclone.{}.tmp", Uuid::new_v4()));
    let result = write(&temporary)
        .and_then(|()| verify_exact_digest(&temporary, digest))
        .and_then(|()| {
            fs::rename(&temporary, &target).map_err(|source| RcloneBinaryError::Io {
                path: target.clone(),
                source,
            })
        });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(target)
}

fn copy_executable(source: &Path, target: &Path, _windows: bool) -> Result<(), RcloneBinaryError> {
    let mut input = File::open(source).map_err(|source_error| RcloneBinaryError::Io {
        path: source.to_owned(),
        source: source_error,
    })?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|source| RcloneBinaryError::Io {
            path: target.to_owned(),
            source,
        })?;
    std::io::copy(&mut input, &mut output).map_err(|source| RcloneBinaryError::Io {
        path: target.to_owned(),
        source,
    })?;
    output.flush().map_err(|source| RcloneBinaryError::Io {
        path: target.to_owned(),
        source,
    })?;
    output.sync_all().map_err(|source| RcloneBinaryError::Io {
        path: target.to_owned(),
        source,
    })?;
    #[cfg(unix)]
    if !_windows {
        let mut permissions = output
            .metadata()
            .map_err(|source| RcloneBinaryError::Io {
                path: target.to_owned(),
                source,
            })?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(target, permissions).map_err(|source| RcloneBinaryError::Io {
            path: target.to_owned(),
            source,
        })?;
    }
    Ok(())
}

fn write_executable_bytes(
    target: &Path,
    payload: &[u8],
    _windows: bool,
) -> Result<(), RcloneBinaryError> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|source| RcloneBinaryError::Io {
            path: target.to_owned(),
            source,
        })?;
    output
        .write_all(payload)
        .and_then(|()| output.flush())
        .and_then(|()| output.sync_all())
        .map_err(|source| RcloneBinaryError::Io {
            path: target.to_owned(),
            source,
        })?;
    #[cfg(unix)]
    if !_windows {
        let mut permissions = output
            .metadata()
            .map_err(|source| RcloneBinaryError::Io {
                path: target.to_owned(),
                source,
            })?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(target, permissions).map_err(|source| RcloneBinaryError::Io {
            path: target.to_owned(),
            source,
        })?;
    }
    Ok(())
}

fn verify_exact_digest(path: &Path, expected: &str) -> Result<(), RcloneBinaryError> {
    let actual = file_sha256(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(RcloneBinaryError::DigestMismatch {
            path: path.to_owned(),
            expected: expected.into(),
            actual,
        })
    }
}

fn newest_valid_materialized(
    directory: &Path,
    windows: bool,
) -> Result<Option<(PathBuf, String)>, RcloneBinaryError> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(None);
    };
    let mut candidates: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok()?.modified().ok()?;
            materialized_digest_prefix(&path, windows).map(|prefix| (path, modified, prefix))
        })
        .collect();
    candidates.sort_by_key(|candidate| Reverse(candidate.1));
    for (path, _, prefix) in candidates {
        let digest = file_sha256(&path)?;
        if digest.starts_with(&prefix) {
            return Ok(Some((path, digest)));
        }
    }
    Ok(None)
}

fn materialized_digest_prefix(path: &Path, windows: bool) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let name = if windows {
        let suffix = name.get(name.len().checked_sub(4)?..)?;
        suffix
            .eq_ignore_ascii_case(".exe")
            .then(|| &name[..name.len() - 4])?
    } else {
        name
    };
    let prefix = name.get(..7)?;
    if !prefix.eq_ignore_ascii_case("rclone-") {
        return None;
    }
    let token = name.get(7..)?;
    (token.len() == HASH_PREFIX_LENGTH && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

pub fn file_sha256(path: &Path) -> Result<String, RcloneBinaryError> {
    let mut file = File::open(path).map_err(|source| RcloneBinaryError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| RcloneBinaryError::Io {
                path: path.to_owned(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn find_in_path(executable: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    env::split_paths(path?).find_map(|directory| {
        let candidate = directory.join(executable);
        candidate.is_file().then_some(candidate)
    })
}

pub fn find_system_executable(executable: &str) -> Option<PathBuf> {
    find_in_path(executable, env::var_os("PATH").as_deref())
}

fn common_paths(executable: &str) -> Vec<PathBuf> {
    let mut paths = directories::BaseDirs::new()
        .map(|dirs| vec![dirs.home_dir().join(".local/bin").join(executable)])
        .unwrap_or_default();
    if !cfg!(windows) {
        paths.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/opt/local/bin",
                "/usr/bin",
                "/snap/bin",
            ]
            .into_iter()
            .map(|directory| Path::new(directory).join(executable)),
        );
    }
    paths
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn file_hashing_does_not_require_a_megabyte_of_stack() {
        const CHILD_PATH: &str = "SSH_MOUNTMATE_SMALL_STACK_HASH_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let digest = std::thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn(move || file_sha256(Path::new(&path)))
                .unwrap()
                .join()
                .unwrap()
                .unwrap();
            assert_eq!(digest.len(), 64);
            return;
        }

        let temp = tempdir().unwrap();
        let path = temp.path().join("two-megabytes.bin");
        fs::write(&path, vec![0x5a; 2 * 1024 * 1024]).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "rclone_binary::tests::file_hashing_does_not_require_a_megabyte_of_stack",
            ])
            .env(CHILD_PATH, &path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn mac_application_resources_are_resolved_from_contents() {
        let executable = Path::new("/Applications/SSH MountMate.app/Contents/MacOS/SSHMountMate");
        let root = application_root(executable);

        assert_eq!(
            root,
            PathBuf::from("/Applications/SSH MountMate.app/Contents")
        );
        assert!(bundled_candidates(&root, false).contains(&root.join("Resources/bin/rclone")));
    }

    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
        }
    }

    #[test]
    fn bundled_binary_requires_and_matches_full_digest() {
        let temp = tempdir().unwrap();
        let app = temp.path().join("app");
        let bundled = app.join("bin").join(binary_name(cfg!(windows)));
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, b"official-rclone").unwrap();

        assert!(matches!(
            resolve_rclone(&paths(temp.path()), &app, None),
            Err(RcloneBinaryError::MissingManifest(_))
        ));
        let digest = file_sha256(&bundled).unwrap();
        fs::write(
            bundled.with_file_name(format!(
                "{}.sha256",
                bundled.file_name().unwrap().to_string_lossy()
            )),
            format!(
                "{digest}  {}\n",
                bundled.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();

        let resolved = resolve_rclone(&paths(temp.path()), &app, None)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, RcloneSource::Bundled);
        assert_eq!(resolved.sha256.as_deref(), Some(digest.as_str()));
        assert_ne!(resolved.path, bundled);
        assert_eq!(fs::read(resolved.path).unwrap(), b"official-rclone");
    }

    #[test]
    fn tampered_content_addressed_binary_is_not_selected() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(paths.managed_bin_dir()).unwrap();
        fs::write(
            paths.managed_bin_dir().join(if cfg!(windows) {
                "rclone-0000000000000000.exe"
            } else {
                "rclone-0000000000000000"
            }),
            b"tampered",
        )
        .unwrap();

        assert_eq!(
            newest_valid_materialized(&paths.managed_bin_dir(), cfg!(windows)).unwrap(),
            None
        );
    }

    #[test]
    fn verified_bytes_are_materialized_once_by_content_digest() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let payload = b"embedded-rclone-test";
        let digest = format!("{:x}", Sha256::digest(payload));
        let first = materialize_verified(&paths, &digest, cfg!(windows), |target| {
            write_executable_bytes(target, payload, cfg!(windows))
        })
        .unwrap();
        let second = materialize_verified(&paths, &digest, cfg!(windows), |_| {
            panic!("an existing verified content-addressed binary must be reused")
        })
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(first).unwrap(), payload);
    }

    #[test]
    fn compiled_embedded_payload_resolves_without_an_application_bundle() {
        if EMBEDDED_RCLONE.is_empty() {
            return;
        }
        let temp = tempdir().unwrap();
        let resolved = resolve_rclone(
            &paths(temp.path()),
            &temp.path().join("standalone-application"),
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.source, RcloneSource::Embedded);
        assert_eq!(resolved.sha256.as_deref(), Some(EMBEDDED_RCLONE_SHA256));
        assert_eq!(file_sha256(&resolved.path).unwrap(), EMBEDDED_RCLONE_SHA256);
    }

    #[test]
    fn configured_file_has_explicit_priority() {
        let temp = tempdir().unwrap();
        let configured = temp.path().join(binary_name(cfg!(windows)));
        fs::write(&configured, b"configured").unwrap();

        let resolved = resolve_rclone(
            &paths(temp.path()),
            &temp.path().join("missing-app"),
            Some(&configured),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.path, configured);
        assert_eq!(resolved.source, RcloneSource::Configured);
    }

    #[test]
    fn materialized_filename_parsing_rejects_non_ascii_boundaries_without_panicking() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("éééé.exe");
        fs::write(&path, b"not rclone").unwrap();
        assert_eq!(materialized_digest_prefix(&path, true), None);
    }
}
