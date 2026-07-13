use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

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
}

pub fn detect_install_layout(executable: &Path) -> Result<InstallLayout, InstallLayoutError> {
    detect_install_layout_for(executable, env::consts::OS, &env::temp_dir())
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
    if !rclone.is_file() || !digest.is_file() {
        return Err(InstallLayoutError::IncompleteDirectoryBundle(
            root.to_owned(),
        ));
    }
    Ok(DirectoryBundleStatus::Complete)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    fn create_file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path)
            .unwrap()
            .write_all(b"binary")
            .unwrap();
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
        let executable = root.join("SSHMountMate.exe");
        create_file(&executable);
        create_file(&root.join("bin/rclone.exe"));
        create_file(&root.join("bin/rclone.exe.sha256"));
        fs::write(
            root.join(DIRECTORY_BUNDLE_MARKER_NAME),
            DIRECTORY_BUNDLE_MARKER,
        )
        .unwrap();

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
