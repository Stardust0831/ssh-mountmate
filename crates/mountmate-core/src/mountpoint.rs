use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::ServerConfig;

pub const HOME_MOUNTPOINT_VALUE: &str = "__home_mnt__";
const WINDOWS_AUTO_DRIVES: &str = "ZYXWVUTSRQPONMLKJIHGFED";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MountpointError {
    #[error("no free Windows drive letter is available")]
    NoFreeDrive,
    #[error("Windows drive is already in use: {0}")]
    DriveInUse(String),
    #[error("mountpoint must be an absolute path or start with ~: {0}")]
    NotAbsolute(PathBuf),
    #[error("Windows folder mountpoint parent does not exist: {0}")]
    ParentMissing(PathBuf),
    #[error("Windows folder mountpoint target must not already exist: {0}")]
    WindowsTargetExists(PathBuf),
    #[error("mountpoint exists but is not a folder: {0}")]
    NotDirectory(PathBuf),
    #[error("mountpoint is already mounted or reserved: {0}")]
    AlreadyReserved(PathBuf),
    #[error("no unique user-folder mountpoint is available under {0}")]
    NoUniqueFolder(PathBuf),
}

pub trait MountpointProbe {
    fn windows_drive_in_use(&self, drive: char) -> bool;
    fn path_exists(&self, path: &Path) -> bool;
    fn path_is_dir(&self, path: &Path) -> bool;
    fn path_is_mount(&self, path: &Path) -> bool;
}

pub struct SystemMountpointProbe;

impl MountpointProbe for SystemMountpointProbe {
    fn windows_drive_in_use(&self, drive: char) -> bool {
        windows_drive_in_use(drive)
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn path_is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn path_is_mount(&self, path: &Path) -> bool {
        system_mountpoint_ready(path)
    }
}

pub fn system_mountpoint_ready(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let value = path.as_os_str().to_string_lossy();
        return windows_drive(&value).map_or_else(|| path.exists(), windows_drive_in_use);
    }
    #[cfg(target_os = "linux")]
    {
        let expected = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return unix_device_changed(path);
        };
        mountinfo.lines().any(|line| {
            line.split_whitespace()
                .nth(4)
                .map(decode_mountinfo_path)
                .is_some_and(|candidate| candidate == expected || candidate == path)
        })
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        unix_device_changed(path)
    }
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_path(value: &str) -> PathBuf {
    PathBuf::from(
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

#[cfg(unix)]
fn unix_device_changed(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = path.parent() else {
        return false;
    };
    match (path.metadata(), parent.metadata()) {
        (Ok(path_metadata), Ok(parent_metadata)) => path_metadata.dev() != parent_metadata.dev(),
        _ => false,
    }
}

#[cfg(windows)]
fn windows_drive_in_use(drive: char) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }

    // GetLogicalDrives has no parameters and returns a bitmask owned by the OS.
    let mask = unsafe { GetLogicalDrives() };
    let drive = drive.to_ascii_uppercase();
    if mask != 0 && drive.is_ascii_alphabetic() {
        mask & (1 << (u32::from(drive) - u32::from('A'))) != 0
    } else {
        PathBuf::from(format!("{drive}:\\")).exists()
    }
}

#[cfg(not(windows))]
fn windows_drive_in_use(drive: char) -> bool {
    PathBuf::from(format!("{drive}:\\")).exists()
}

pub struct MountpointAllocator<'a> {
    home: PathBuf,
    windows: bool,
    probe: &'a dyn MountpointProbe,
    reserved: HashSet<String>,
}

impl<'a> MountpointAllocator<'a> {
    pub fn new(home: PathBuf, windows: bool, probe: &'a dyn MountpointProbe) -> Self {
        Self {
            home,
            windows,
            probe,
            reserved: HashSet::new(),
        }
    }

    pub fn reserve(&mut self, path: &Path) {
        self.reserved.insert(path_key(path, self.windows));
    }

    pub fn resolve(&mut self, server: &ServerConfig) -> Result<PathBuf, MountpointError> {
        let configured = server.mountpoint.trim();
        let path = if configured.is_empty() || configured.eq_ignore_ascii_case("auto") {
            if self.windows {
                self.allocate_windows_drive()?
            } else {
                self.allocate_home_folder(server)?
            }
        } else if configured == HOME_MOUNTPOINT_VALUE {
            self.allocate_home_folder(server)?
        } else if let Some(drive) = windows_drive(configured) {
            let path = PathBuf::from(format!("{drive}:"));
            if self.probe.windows_drive_in_use(drive) || self.is_reserved(&path) {
                return Err(MountpointError::DriveInUse(path.display().to_string()));
            }
            path
        } else {
            let path = expand_home(Path::new(configured), &self.home);
            self.validate_custom(&path)?;
            path
        };
        self.reserve(&path);
        Ok(path)
    }

    fn allocate_windows_drive(&self) -> Result<PathBuf, MountpointError> {
        WINDOWS_AUTO_DRIVES
            .chars()
            .map(|drive| (drive, PathBuf::from(format!("{drive}:"))))
            .find(|(drive, path)| {
                !self.probe.windows_drive_in_use(*drive) && !self.is_reserved(path)
            })
            .map(|(_, path)| path)
            .ok_or(MountpointError::NoFreeDrive)
    }

    fn allocate_home_folder(&self, server: &ServerConfig) -> Result<PathBuf, MountpointError> {
        let root = stable_folder_name(server.display_name());
        let parent = self.home.join("mnt");
        for index in 1..1000 {
            let name = if index == 1 {
                root.clone()
            } else {
                format!("{root}-{index}")
            };
            let candidate = parent.join(name);
            if self.is_reserved(&candidate) || self.probe.path_is_mount(&candidate) {
                continue;
            }
            if self.windows && self.probe.path_exists(&candidate) {
                continue;
            }
            if !self.windows
                && self.probe.path_exists(&candidate)
                && !self.probe.path_is_dir(&candidate)
            {
                continue;
            }
            return Ok(candidate);
        }
        Err(MountpointError::NoUniqueFolder(parent))
    }

    fn validate_custom(&self, path: &Path) -> Result<(), MountpointError> {
        if !path_is_absolute(path, self.windows) {
            return Err(MountpointError::NotAbsolute(path.to_owned()));
        }
        if self.is_reserved(path) || self.probe.path_is_mount(path) {
            return Err(MountpointError::AlreadyReserved(path.to_owned()));
        }
        if self.windows {
            let parent = windows_parent(path);
            if !self.probe.path_exists(&parent) {
                return Err(MountpointError::ParentMissing(parent));
            }
            if self.probe.path_exists(path) {
                return Err(MountpointError::WindowsTargetExists(path.to_owned()));
            }
        } else if self.probe.path_exists(path) && !self.probe.path_is_dir(path) {
            return Err(MountpointError::NotDirectory(path.to_owned()));
        }
        Ok(())
    }

    fn is_reserved(&self, path: &Path) -> bool {
        self.reserved.contains(&path_key(path, self.windows))
    }
}

fn windows_drive(value: &str) -> Option<char> {
    let bytes = value.as_bytes();
    (matches!(bytes, [_, b':'] | [_, b':', b'\\' | b'/']) && bytes[0].is_ascii_alphabetic())
        .then(|| char::from(bytes[0]).to_ascii_uppercase())
}

fn windows_parent(path: &Path) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    value
        .rfind(['\\', '/'])
        .filter(|index| *index > 2)
        .map(|index| PathBuf::from(&value[..index]))
        .unwrap_or_else(|| path.parent().unwrap_or_else(|| Path::new("")).to_owned())
}

fn path_is_absolute(path: &Path, windows: bool) -> bool {
    if !windows {
        return path.is_absolute();
    }
    let value = path.to_string_lossy();
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || value.starts_with("\\\\")
        || value.starts_with("//")
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        home.to_owned()
    } else if value.starts_with("~/") || value.starts_with("~\\") {
        home.join(&value[2..])
    } else {
        path.to_owned()
    }
}

fn stable_folder_name(value: &str) -> String {
    let name: String = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_matches(['.', '_', '-']);
    if name.is_empty() {
        "Server".into()
    } else {
        name.into()
    }
}

fn path_key(path: &Path, windows: bool) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if windows {
        value.trim_end_matches('/').to_ascii_lowercase()
    } else {
        value.trim_end_matches('/').into()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct FakeProbe {
        drives: HashSet<char>,
        existing: HashSet<String>,
        directories: HashSet<String>,
        mounts: HashSet<String>,
        observed: RefCell<Vec<PathBuf>>,
    }

    impl MountpointProbe for FakeProbe {
        fn windows_drive_in_use(&self, drive: char) -> bool {
            self.drives.contains(&drive)
        }

        fn path_exists(&self, path: &Path) -> bool {
            self.observed.borrow_mut().push(path.to_owned());
            self.existing.contains(&path_key(path, true))
        }

        fn path_is_dir(&self, path: &Path) -> bool {
            self.directories.contains(&path_key(path, true))
        }

        fn path_is_mount(&self, path: &Path) -> bool {
            self.mounts.contains(&path_key(path, true))
        }
    }

    fn server(mountpoint: &str) -> ServerConfig {
        ServerConfig {
            id: "alpha".into(),
            name: "Alpha Server".into(),
            mountpoint: mountpoint.into(),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn batch_auto_drive_allocation_never_reuses_a_letter() {
        let probe = FakeProbe {
            drives: HashSet::from(['Z']),
            ..FakeProbe::default()
        };
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(allocator.resolve(&server("")).unwrap(), PathBuf::from("Y:"));
        assert_eq!(
            allocator.resolve(&server("Auto")).unwrap(),
            PathBuf::from("X:")
        );
    }

    #[test]
    fn home_folder_is_stable_and_uniquely_reserved() {
        let probe = FakeProbe::default();
        let mut allocator = MountpointAllocator::new(PathBuf::from("/home/me"), false, &probe);

        assert_eq!(
            allocator.resolve(&server(HOME_MOUNTPOINT_VALUE)).unwrap(),
            PathBuf::from("/home/me/mnt/Alpha_Server")
        );
        assert_eq!(
            allocator.resolve(&server(HOME_MOUNTPOINT_VALUE)).unwrap(),
            PathBuf::from("/home/me/mnt/Alpha_Server-2")
        );
    }

    #[test]
    fn windows_folder_requires_existing_parent_and_missing_target() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("c:/mounts".into());
        probe.directories.insert("c:/mounts".into());
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("C:\\mounts\\alpha")).unwrap(),
            PathBuf::from("C:\\mounts\\alpha")
        );
    }

    #[test]
    fn custom_home_path_expands_before_validation() {
        let probe = FakeProbe::default();
        let mut allocator = MountpointAllocator::new(PathBuf::from("/home/me"), false, &probe);

        assert_eq!(
            allocator.resolve(&server("~/custom/alpha")).unwrap(),
            PathBuf::from("/home/me/custom/alpha")
        );
    }
}
