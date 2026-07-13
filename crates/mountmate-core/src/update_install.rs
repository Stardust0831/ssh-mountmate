use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;
use uuid::Uuid;

use crate::rclone_binary::verify_bundled;

const DIRECTORY_BUNDLE_MARKER_NAME: &str = "SSHMountMate.install-layout";
const DIRECTORY_BUNDLE_MARKER: &[u8] = b"ssh-mountmate-directory-bundle-v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    StandaloneExecutable,
    DirectoryBundle,
    MacApplicationBundle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLayout {
    pub kind: InstallKind,
    pub executable: PathBuf,
    /// The file or directory replaced as one unit during an update.
    pub replace_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPayload {
    pub root: PathBuf,
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPaths {
    pub prepared: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Error)]
pub enum InstallLayoutError {
    #[error("could not inspect the running executable at {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the running executable is a symbolic link, which cannot be updated safely: {0}")]
    SymlinkExecutable(PathBuf),
    #[error("the running executable is not a regular file: {0}")]
    NotExecutableFile(PathBuf),
    #[error(
        "automatic updates are disabled when running from the Windows temporary directory: {0}"
    )]
    TemporaryWindowsLocation(PathBuf),
    #[error("the executable is inside a malformed macOS application bundle: {0}")]
    MalformedMacApplication(PathBuf),
    #[error("the SSH MountMate directory-bundle marker is invalid: {0}")]
    InvalidDirectoryBundleMarker(PathBuf),
    #[error("the SSH MountMate directory bundle is incomplete: {0}")]
    IncompleteDirectoryBundle(PathBuf),
    #[error("the bundled rclone failed verification at {path}: {message}")]
    CorruptDirectoryBundle { path: PathBuf, message: String },
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("could not inspect the extracted update at {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the extracted update root is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("the extracted update did not contain a complete SSH MountMate directory bundle")]
    MissingDirectoryBundle,
    #[error("the extracted update contained more than one SSH MountMate directory bundle")]
    AmbiguousDirectoryBundle,
    #[error("the update bundle does not contain the expected executable: {0}")]
    MissingExecutable(PathBuf),
    #[error(transparent)]
    Layout(#[from] InstallLayoutError),
}

#[derive(Debug, Error)]
pub enum TransactionPlanError {
    #[error("the installation does not have a replaceable parent directory: {0}")]
    MissingParent(PathBuf),
    #[error("the installation path does not have a usable file name: {0}")]
    MissingName(PathBuf),
    #[error("a previous update backup still exists and must be recovered first: {0}")]
    BackupExists(PathBuf),
    #[error("the generated update staging path already exists: {0}")]
    PreparedExists(PathBuf),
    #[error("could not inspect update transaction path {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn detect_install_layout(executable: &Path) -> Result<InstallLayout, InstallLayoutError> {
    detect_install_layout_for(executable, env::consts::OS, &env::temp_dir())
}

pub fn locate_directory_payload(
    extracted_root: &Path,
    os: &str,
) -> Result<DirectoryPayload, PayloadError> {
    let metadata =
        fs::symlink_metadata(extracted_root).map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PayloadError::NotDirectory(extracted_root.to_owned()));
    }
    let extracted_root = extracted_root
        .canonicalize()
        .map_err(|source| PayloadError::Inspect {
            path: extracted_root.to_owned(),
            source,
        })?;

    let mut candidates = Vec::new();
    collect_directory_payload_candidate(&extracted_root, os, &mut candidates)?;
    for entry in fs::read_dir(&extracted_root).map_err(|source| PayloadError::Inspect {
        path: extracted_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| PayloadError::Inspect {
            path: extracted_root.clone(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| PayloadError::Inspect {
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_directory_payload_candidate(&entry.path(), os, &mut candidates)?;
        }
    }

    match candidates.len() {
        0 => Err(PayloadError::MissingDirectoryBundle),
        1 => Ok(candidates.remove(0)),
        _ => Err(PayloadError::AmbiguousDirectoryBundle),
    }
}

pub fn plan_transaction_paths(
    layout: &InstallLayout,
) -> Result<TransactionPaths, TransactionPlanError> {
    let parent = layout
        .replace_path
        .parent()
        .ok_or_else(|| TransactionPlanError::MissingParent(layout.replace_path.clone()))?;
    let name = layout
        .replace_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TransactionPlanError::MissingName(layout.replace_path.clone()))?;
    let backup = parent.join(format!(".{name}.ssh-mountmate-backup"));
    if path_entry_exists(&backup)? {
        return Err(TransactionPlanError::BackupExists(backup));
    }
    let prepared = parent.join(format!(
        ".{name}.ssh-mountmate-prepared-{}",
        Uuid::new_v4().simple()
    ));
    if path_entry_exists(&prepared)? {
        return Err(TransactionPlanError::PreparedExists(prepared));
    }
    Ok(TransactionPaths { prepared, backup })
}

fn detect_install_layout_for(
    executable: &Path,
    os: &str,
    temporary_directory: &Path,
) -> Result<InstallLayout, InstallLayoutError> {
    let metadata =
        fs::symlink_metadata(executable).map_err(|source| InstallLayoutError::Inspect {
            path: executable.to_owned(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(InstallLayoutError::SymlinkExecutable(executable.to_owned()));
    }
    if !metadata.is_file() {
        return Err(InstallLayoutError::NotExecutableFile(executable.to_owned()));
    }
    let executable = executable
        .canonicalize()
        .map_err(|source| InstallLayoutError::Inspect {
            path: executable.to_owned(),
            source,
        })?;

    if os == "windows" && path_is_within(&executable, temporary_directory, true) {
        return Err(InstallLayoutError::TemporaryWindowsLocation(executable));
    }

    if os == "macos" {
        if let Some(application) = mac_application_root(&executable) {
            return Ok(InstallLayout {
                kind: InstallKind::MacApplicationBundle,
                executable,
                replace_path: application,
            });
        }
        if executable
            .ancestors()
            .any(|path| path.extension().is_some_and(|value| value == "app"))
        {
            return Err(InstallLayoutError::MalformedMacApplication(executable));
        }
    }

    let parent = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_owned();
    match directory_bundle_status(&parent, os)? {
        DirectoryBundleStatus::Complete => Ok(InstallLayout {
            kind: InstallKind::DirectoryBundle,
            executable,
            replace_path: parent,
        }),
        DirectoryBundleStatus::NotBundle => Ok(InstallLayout {
            kind: InstallKind::StandaloneExecutable,
            replace_path: executable.clone(),
            executable,
        }),
    }
}

fn path_is_within(path: &Path, directory: &Path, case_insensitive: bool) -> bool {
    directory
        .canonicalize()
        .is_ok_and(|directory| path_components_start_with(path, &directory, case_insensitive))
}

fn path_components_start_with(path: &Path, directory: &Path, case_insensitive: bool) -> bool {
    let mut path_components = path.components();
    directory.components().all(|expected| {
        path_components.next().is_some_and(|actual| {
            if case_insensitive {
                actual.as_os_str().to_string_lossy().to_lowercase()
                    == expected.as_os_str().to_string_lossy().to_lowercase()
            } else {
                actual == expected
            }
        })
    })
}

fn path_entry_exists(path: &Path) -> Result<bool, TransactionPlanError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(TransactionPlanError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

fn is_regular_file_without_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn mac_application_root(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let application = contents.parent()?;
    (application.extension()? == "app").then(|| application.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryBundleStatus {
    NotBundle,
    Complete,
}

fn directory_bundle_status(
    root: &Path,
    os: &str,
) -> Result<DirectoryBundleStatus, InstallLayoutError> {
    let marker = root.join(DIRECTORY_BUNDLE_MARKER_NAME);
    let marker_metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DirectoryBundleStatus::NotBundle);
        }
        Err(source) => {
            return Err(InstallLayoutError::Inspect {
                path: marker,
                source,
            });
        }
    };
    if !marker_metadata.is_file()
        || marker_metadata.file_type().is_symlink()
        || marker_metadata.len() != DIRECTORY_BUNDLE_MARKER.len() as u64
        || !fs::read(&marker).is_ok_and(|contents| contents == DIRECTORY_BUNDLE_MARKER)
    {
        return Err(InstallLayoutError::InvalidDirectoryBundleMarker(marker));
    }

    let binary = if os == "windows" {
        "rclone.exe"
    } else {
        "rclone"
    };
    let rclone = root.join("bin").join(binary);
    let digest = rclone.with_file_name(format!("{binary}.sha256"));
    if !is_regular_file_without_symlink(&rclone) || !is_regular_file_without_symlink(&digest) {
        return Err(InstallLayoutError::IncompleteDirectoryBundle(
            root.to_owned(),
        ));
    }
    verify_bundled(&rclone).map_err(|error| InstallLayoutError::CorruptDirectoryBundle {
        path: rclone,
        message: error.to_string(),
    })?;
    Ok(DirectoryBundleStatus::Complete)
}

fn collect_directory_payload_candidate(
    root: &Path,
    os: &str,
    candidates: &mut Vec<DirectoryPayload>,
) -> Result<(), PayloadError> {
    if directory_bundle_status(root, os)? == DirectoryBundleStatus::NotBundle {
        return Ok(());
    }
    let executable = root.join(if os == "windows" {
        "SSHMountMate.exe"
    } else {
        "SSHMountMate"
    });
    let metadata = match fs::symlink_metadata(&executable) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PayloadError::MissingExecutable(executable));
        }
        Err(source) => {
            return Err(PayloadError::Inspect {
                path: executable,
                source,
            });
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PayloadError::MissingExecutable(executable));
    }
    candidates.push(DirectoryPayload {
        root: root.to_owned(),
        executable,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::*;

    fn create_file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path)
            .unwrap()
            .write_all(b"binary")
            .unwrap();
    }

    fn create_directory_bundle(root: &Path, os: &str) -> PathBuf {
        let executable = root.join(if os == "windows" {
            "SSHMountMate.exe"
        } else {
            "SSHMountMate"
        });
        let rclone_name = if os == "windows" {
            "rclone.exe"
        } else {
            "rclone"
        };
        let rclone = root.join("bin").join(rclone_name);
        create_file(&executable);
        create_file(&rclone);
        fs::write(
            rclone.with_file_name(format!("{rclone_name}.sha256")),
            format!("{:x}", Sha256::digest(b"binary")),
        )
        .unwrap();
        fs::write(
            root.join(DIRECTORY_BUNDLE_MARKER_NAME),
            DIRECTORY_BUNDLE_MARKER,
        )
        .unwrap();
        executable
    }

    #[test]
    fn standalone_executable_replaces_only_itself() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("portable/SSHMountMate.exe");
        create_file(&executable);

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("unrelated-temp"))
                .unwrap();

        assert_eq!(layout.kind, InstallKind::StandaloneExecutable);
        assert_eq!(layout.replace_path, executable.canonicalize().unwrap());
    }

    #[test]
    fn complete_directory_bundle_replaces_the_bundle_directory() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "windows");

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("other")).unwrap();

        assert_eq!(layout.kind, InstallKind::DirectoryBundle);
        assert_eq!(layout.replace_path, root.canonicalize().unwrap());
    }

    #[test]
    fn adjacent_rclone_without_an_install_marker_does_not_claim_the_directory() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("user-folder");
        let executable = root.join("renamed.exe");
        create_file(&executable);
        create_file(&root.join("bin/rclone.exe"));
        create_file(&root.join("bin/rclone.exe.sha256"));

        let layout =
            detect_install_layout_for(&executable, "windows", &temp.path().join("other")).unwrap();

        assert_eq!(layout.kind, InstallKind::StandaloneExecutable);
        assert_eq!(layout.replace_path, executable.canonicalize().unwrap());
    }

    #[test]
    fn marked_but_incomplete_directory_bundle_is_rejected() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = root.join("SSHMountMate");
        create_file(&executable);
        fs::write(
            root.join(DIRECTORY_BUNDLE_MARKER_NAME),
            DIRECTORY_BUNDLE_MARKER,
        )
        .unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::IncompleteDirectoryBundle(_))
        ));
    }

    #[test]
    fn marked_directory_bundle_with_tampered_rclone_is_rejected() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "linux");
        fs::write(root.join("bin/rclone"), b"tampered").unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::CorruptDirectoryBundle { .. })
        ));
    }

    #[test]
    fn directory_payload_accepts_root_and_single_wrapper_layouts() {
        for wrapped in [false, true] {
            let temp = tempdir().unwrap();
            let extracted = temp.path().join("extracted");
            let bundle = if wrapped {
                extracted.join("SSHMountMate")
            } else {
                extracted.clone()
            };
            fs::create_dir_all(&extracted).unwrap();
            let executable = create_directory_bundle(&bundle, "windows");

            let payload = locate_directory_payload(&extracted, "windows").unwrap();

            assert_eq!(payload.root, bundle.canonicalize().unwrap());
            assert_eq!(payload.executable, executable.canonicalize().unwrap());
        }
    }

    #[test]
    fn directory_payload_rejects_ambiguous_bundles() {
        let temp = tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        create_directory_bundle(&extracted.join("first"), "linux");
        create_directory_bundle(&extracted.join("second"), "linux");

        assert!(matches!(
            locate_directory_payload(&extracted, "linux"),
            Err(PayloadError::AmbiguousDirectoryBundle)
        ));
    }

    #[test]
    fn transaction_paths_are_siblings_and_existing_backups_block_updates() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();

        let transaction = plan_transaction_paths(&layout).unwrap();
        assert_eq!(transaction.prepared.parent(), executable.parent());
        assert_eq!(transaction.backup.parent(), executable.parent());
        assert_ne!(transaction.prepared, transaction.backup);

        create_file(&transaction.backup);
        assert!(matches!(
            plan_transaction_paths(&layout),
            Err(TransactionPlanError::BackupExists(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn broken_backup_symlinks_still_block_update_transactions() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let executable = temp.path().join("SSHMountMate");
        create_file(&executable);
        let layout = detect_install_layout_for(&executable, "linux", temp.path()).unwrap();
        let transaction = plan_transaction_paths(&layout).unwrap();
        symlink(
            temp.path().join("missing-backup-target"),
            &transaction.backup,
        )
        .unwrap();

        assert!(matches!(
            plan_transaction_paths(&layout),
            Err(TransactionPlanError::BackupExists(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn directory_bundle_rejects_a_symlinked_rclone() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let root = temp.path().join("SSHMountMate");
        let executable = create_directory_bundle(&root, "linux");
        let rclone = root.join("bin/rclone");
        let external = temp.path().join("external-rclone");
        create_file(&external);
        fs::remove_file(&rclone).unwrap();
        symlink(external, rclone).unwrap();

        assert!(matches!(
            detect_install_layout_for(&executable, "linux", temp.path()),
            Err(InstallLayoutError::IncompleteDirectoryBundle(_))
        ));
    }

    #[test]
    fn mac_application_replaces_the_entire_app_bundle() {
        let temp = tempdir().unwrap();
        let application = temp.path().join("SSH MountMate.app");
        let executable = application.join("Contents/MacOS/SSHMountMate");
        create_file(&executable);

        let layout = detect_install_layout_for(&executable, "macos", temp.path()).unwrap();

        assert_eq!(layout.kind, InstallKind::MacApplicationBundle);
        assert_eq!(layout.replace_path, application.canonicalize().unwrap());
    }

    #[test]
    fn malformed_mac_application_is_not_treated_as_a_directory_bundle() {
        let temp = tempdir().unwrap();
        let executable = temp
            .path()
            .join("SSH MountMate.app/Unexpected/SSHMountMate");
        create_file(&executable);

        assert!(matches!(
            detect_install_layout_for(&executable, "macos", temp.path()),
            Err(InstallLayoutError::MalformedMacApplication(_))
        ));
    }

    #[test]
    fn windows_temporary_extractions_are_rejected() {
        let temp = tempdir().unwrap();
        let temporary_directory = temp.path().join("Temp");
        let executable = temporary_directory.join("archive/SSHMountMate.exe");
        create_file(&executable);

        assert!(matches!(
            detect_install_layout_for(&executable, "windows", &temporary_directory),
            Err(InstallLayoutError::TemporaryWindowsLocation(_))
        ));
    }

    #[test]
    fn windows_containment_is_case_insensitive_but_component_aware() {
        assert!(path_components_start_with(
            Path::new("C:/Users/Agent/AppData/Local/Temp/archive/app.exe"),
            Path::new("c:/users/agent/appdata/local/temp"),
            true,
        ));
        assert!(!path_components_start_with(
            Path::new("C:/Users/Agent/AppData/Local/Temporary/app.exe"),
            Path::new("c:/users/agent/appdata/local/temp"),
            true,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_launchers_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let target = temp.path().join("SSHMountMate-real");
        let launcher = temp.path().join("SSHMountMate");
        create_file(&target);
        symlink(target, &launcher).unwrap();

        assert!(matches!(
            detect_install_layout_for(&launcher, "linux", temp.path()),
            Err(InstallLayoutError::SymlinkExecutable(_))
        ));
    }
}
