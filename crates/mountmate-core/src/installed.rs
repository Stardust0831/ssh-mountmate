//! Identity and install-policy primitives for the Windows installed edition.
//!
//! Registry access intentionally lives in `mountmate-platform`.  This module
//! validates the registry record against the canonical fixed path and owns the
//! version/uninstall policy so it can be tested on every host platform.

use std::path::{Path, PathBuf};

use semver::Version;
use thiserror::Error;

pub const INSTALLED_MARKER_SCHEMA_VERSION: u32 = 1;
pub const WINDOWS_INSTALL_RECORD_KEY: &str = r"Software\Stardust\SSH MountMate\Install";
pub const WINDOWS_INSTALL_DIRECTORY: &str = r"Programs\SSH MountMate";
pub const WINDOWS_EXECUTABLE_NAME: &str = "SSHMountMate.exe";
pub const WINDOWS_AUMID: &str = "Stardust.SSHMountMate";

/// Values written by the per-user installer under the HKCU install record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledInstallRecord {
    pub schema_version: u32,
    pub version: String,
    pub install_root: PathBuf,
    pub executable_path: PathBuf,
    pub aumid: String,
    pub architecture: String,
}

/// A registry record that has been checked against both the current executable
/// and the canonical `%LOCALAPPDATA%` install location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEditionIdentity {
    pub version: Version,
    pub install_root: PathBuf,
    pub executable_path: PathBuf,
    pub aumid: String,
    pub architecture: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InstalledIdentityError {
    #[error("installed identity marker schema {0} is unsupported")]
    UnsupportedSchema(u32),
    #[error("installed identity marker has an invalid version: {0}")]
    InvalidVersion(String),
    #[error("installed identity marker has an invalid AUMID")]
    InvalidAumid,
    #[error("installed identity marker does not use the canonical install root")]
    InstallRootMismatch,
    #[error("installed identity marker does not use the canonical executable path")]
    ExecutablePathMismatch,
    #[error("current executable is not the canonical installed executable")]
    CurrentExecutableMismatch,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InstallPolicyError {
    #[error("installed version {existing} is newer than requested version {requested}; refusing implicit downgrade")]
    DowngradeBlocked { existing: Version, requested: Version },
    #[error("invalid installed version: {0}")]
    InvalidExistingVersion(String),
    #[error("invalid requested version: {0}")]
    InvalidRequestedVersion(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UninstallPreflightError {
    #[error("SSH MountMate has active mounts; unmount them before uninstalling")]
    ActiveMounts,
}

/// Build the only supported installed-edition paths for a Windows user.
pub fn canonical_windows_paths(local_app_data: &Path) -> (PathBuf, PathBuf) {
    let root = local_app_data.join(WINDOWS_INSTALL_DIRECTORY);
    (root.clone(), root.join(WINDOWS_EXECUTABLE_NAME))
}

/// Validate an HKCU marker and the process path together.  A path that merely
/// happens to be under `%LOCALAPPDATA%` is not considered installed without a
/// matching marker.
pub fn validate_installed_identity(
    record: &InstalledInstallRecord,
    current_executable: &Path,
    local_app_data: &Path,
) -> Result<InstalledEditionIdentity, InstalledIdentityError> {
    if record.schema_version != INSTALLED_MARKER_SCHEMA_VERSION {
        return Err(InstalledIdentityError::UnsupportedSchema(
            record.schema_version,
        ));
    }
    let version = Version::parse(&record.version)
        .map_err(|_| InstalledIdentityError::InvalidVersion(record.version.clone()))?;
    if version.to_string() != record.version {
        return Err(InstalledIdentityError::InvalidVersion(record.version.clone()));
    }
    if record.aumid != WINDOWS_AUMID {
        return Err(InstalledIdentityError::InvalidAumid);
    }

    let (canonical_root, canonical_executable) = canonical_windows_paths(local_app_data);
    if !same_windows_path(&record.install_root, &canonical_root) {
        return Err(InstalledIdentityError::InstallRootMismatch);
    }
    if !same_windows_path(&record.executable_path, &canonical_executable) {
        return Err(InstalledIdentityError::ExecutablePathMismatch);
    }
    if !same_windows_path(current_executable, &canonical_executable) {
        return Err(InstalledIdentityError::CurrentExecutableMismatch);
    }

    Ok(InstalledEditionIdentity {
        version,
        install_root: canonical_root,
        executable_path: canonical_executable,
        aumid: record.aumid.clone(),
        architecture: record.architecture.clone(),
    })
}

/// Return an error when installing an older version over an installed edition.
/// Equal versions are accepted so repair/reinstall remains possible.
pub fn enforce_no_downgrade(
    existing_version: Option<&str>,
    requested_version: &str,
) -> Result<(), InstallPolicyError> {
    let requested = Version::parse(requested_version)
        .map_err(|_| InstallPolicyError::InvalidRequestedVersion(requested_version.into()))?;
    if requested.to_string() != requested_version {
        return Err(InstallPolicyError::InvalidRequestedVersion(
            requested_version.into(),
        ));
    }
    let Some(existing_version) = existing_version else {
        return Ok(());
    };
    let existing = Version::parse(existing_version)
        .map_err(|_| InstallPolicyError::InvalidExistingVersion(existing_version.into()))?;
    if existing.to_string() != existing_version {
        return Err(InstallPolicyError::InvalidExistingVersion(
            existing_version.into(),
        ));
    }
    if existing > requested {
        return Err(InstallPolicyError::DowngradeBlocked { existing, requested });
    }
    Ok(())
}

/// Alias suitable for installer/platform callers.
pub fn check_install_version(
    existing_version: Option<&str>,
    requested_version: &str,
) -> Result<(), InstallPolicyError> {
    enforce_no_downgrade(existing_version, requested_version)
}

/// Common uninstall hook used by the app's future installer preflight CLI.
pub fn enforce_uninstall_preflight(active_mounts: bool) -> Result<(), UninstallPreflightError> {
    if active_mounts {
        Err(UninstallPreflightError::ActiveMounts)
    } else {
        Ok(())
    }
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    windows_path_key(left) == windows_path_key(right)
}

fn windows_path_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().replace('/', "\\");
    while key.ends_with('\\') && key.len() > 3 {
        key.pop();
    }
    key.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(local_app_data: &Path, version: &str) -> InstalledInstallRecord {
        let (root, executable) = canonical_windows_paths(local_app_data);
        InstalledInstallRecord {
            schema_version: INSTALLED_MARKER_SCHEMA_VERSION,
            version: version.into(),
            install_root: root,
            executable_path: executable,
            aumid: WINDOWS_AUMID.into(),
            architecture: "x64".into(),
        }
    }

    #[test]
    fn installed_identity_requires_marker_and_canonical_path() {
        let local = Path::new(r"C:\Users\alice\AppData\Local");
        let marker = record(local, "0.6.0-alpha.1");
        let current = Path::new(
            r"c:/users/alice/appdata/local/programs/ssh mountmate/SSHMountMate.exe",
        );
        assert!(validate_installed_identity(&marker, current, local).is_ok());
        assert_eq!(
            validate_installed_identity(&marker, Path::new(r"C:\tmp\SSHMountMate.exe"), local),
            Err(InstalledIdentityError::CurrentExecutableMismatch)
        );
    }

    #[test]
    fn marker_path_mismatch_is_rejected_even_when_process_path_is_canonical() {
        let local = Path::new(r"C:\Users\alice\AppData\Local");
        let mut marker = record(local, "0.6.0-alpha.1");
        marker.executable_path = PathBuf::from(r"C:\Users\alice\Desktop\SSHMountMate.exe");
        let canonical = canonical_windows_paths(local).1;
        assert_eq!(
            validate_installed_identity(&marker, &canonical, local),
            Err(InstalledIdentityError::ExecutablePathMismatch)
        );
    }

    #[test]
    fn higher_installed_version_blocks_downgrade_but_equal_reinstall_is_allowed() {
        assert!(enforce_no_downgrade(Some("0.6.0-alpha.2"), "0.6.0-alpha.1").is_err());
        assert!(enforce_no_downgrade(Some("0.6.0-alpha.1"), "0.6.0-alpha.1").is_ok());
        assert!(enforce_no_downgrade(Some("0.5.0"), "0.6.0-alpha.1").is_ok());
    }

    #[test]
    fn uninstall_preflight_blocks_active_mounts() {
        assert_eq!(
            enforce_uninstall_preflight(true),
            Err(UninstallPreflightError::ActiveMounts)
        );
        assert!(enforce_uninstall_preflight(false).is_ok());
    }
}
