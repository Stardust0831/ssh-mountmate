use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::ServerConfig;

pub const HOME_MOUNTPOINT_VALUE: &str = "__home_mnt__";
const WINDOWS_AUTO_DRIVES: &str = "ZYXWVUTSRQPONMLKJIHGFED";

/// The volume type reported by Windows for a folder mountpoint's backing root.
///
/// WinFsp directory mountpoints are only supported on fixed local volumes.  The
/// remaining values are retained separately so callers can provide a useful
/// preflight error instead of allowing rclone to fail after it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsVolumeKind {
    Fixed,
    Mounted,
    Reparse,
    NonNtfs,
    Remote,
    Removable,
    CdRom,
    RamDisk,
    Unknown,
    NoRoot,
    Missing,
}

impl std::fmt::Display for WindowsVolumeKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Fixed => "fixed local",
            Self::Mounted => "mounted volume",
            Self::Reparse => "reparse-backed",
            Self::NonNtfs => "non-NTFS",
            Self::Remote => "remote",
            Self::Removable => "removable",
            Self::CdRom => "CD-ROM",
            Self::RamDisk => "RAM disk",
            Self::Unknown => "unknown",
            Self::NoRoot => "no root",
            Self::Missing => "missing",
        };
        formatter.write_str(value)
    }
}

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
    #[error("Windows folder mountpoint cannot use a UNC path: {0}")]
    WindowsUncPath(PathBuf),
    #[error("Windows folder mountpoint volume could not be resolved: {0}")]
    WindowsVolumeMissing(PathBuf),
    #[error("Windows folder mountpoint volume is unsupported ({kind}): {path}")]
    WindowsVolumeUnsupported {
        path: PathBuf,
        kind: WindowsVolumeKind,
    },
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

    /// Resolve the backing volume for a Windows directory mountpoint.  The
    /// default keeps third-party probes source-compatible while making an
    /// unresolved volume fail closed on Windows.
    fn windows_volume_kind(&self, _path: &Path) -> WindowsVolumeKind {
        WindowsVolumeKind::Missing
    }
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

    fn windows_volume_kind(&self, path: &Path) -> WindowsVolumeKind {
        system_windows_volume_kind(path)
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

#[cfg(windows)]
fn system_windows_volume_kind(path: &Path) -> WindowsVolumeKind {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, GetDriveTypeW,
        GetFileInformationByHandleEx, GetVolumeInformationW, GetVolumePathNameW, OPEN_EXISTING,
    };

    // GetDriveTypeW's documented return values are stable Win32 ABI values.
    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    let mut input: Vec<u16> = path.as_os_str().encode_wide().collect();
    input.push(0);
    // MAX_PATH is insufficient for extended-length paths.  GetVolumePathNameW
    // accepts a caller-provided buffer, so use the documented maximum path
    // length and leave room for the terminating NUL.
    let mut volume = vec![0u16; 32_768];
    let resolved =
        unsafe { GetVolumePathNameW(input.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) };
    if resolved == 0 {
        return WindowsVolumeKind::Missing;
    }
    let volume_end = volume
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(volume.len());
    if !ordinary_windows_volume_root(&volume[..volume_end], path) {
        return WindowsVolumeKind::Mounted;
    }
    match unsafe { GetDriveTypeW(volume.as_ptr()) } {
        DRIVE_FIXED => {}
        DRIVE_REMOTE => return WindowsVolumeKind::Remote,
        DRIVE_REMOVABLE => return WindowsVolumeKind::Removable,
        DRIVE_CDROM => return WindowsVolumeKind::CdRom,
        DRIVE_RAMDISK => return WindowsVolumeKind::RamDisk,
        DRIVE_NO_ROOT_DIR => return WindowsVolumeKind::NoRoot,
        DRIVE_UNKNOWN => return WindowsVolumeKind::Unknown,
        _ => return WindowsVolumeKind::Unknown,
    }
    for prefix in existing_windows_prefixes(path) {
        let mut wide: Vec<u16> = prefix.as_os_str().encode_wide().collect();
        wide.push(0);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return WindowsVolumeKind::Missing;
        }
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        let inspected = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        unsafe { CloseHandle(handle) };
        if inspected == 0 {
            return WindowsVolumeKind::Missing;
        }
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return WindowsVolumeKind::Reparse;
        }
    }
    let mut filesystem = [0u16; 64];
    let volume_read = unsafe {
        GetVolumeInformationW(
            volume.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if volume_read == 0 {
        return WindowsVolumeKind::Missing;
    }
    let filesystem_end = filesystem
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(filesystem.len());
    if !String::from_utf16_lossy(&filesystem[..filesystem_end]).eq_ignore_ascii_case("NTFS") {
        return WindowsVolumeKind::NonNtfs;
    }
    WindowsVolumeKind::Fixed
}

#[cfg(windows)]
fn existing_windows_prefixes(path: &Path) -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if current.has_root() && current.exists() {
            prefixes.push(current.clone());
        }
    }
    prefixes
}

#[cfg(windows)]
fn ordinary_windows_volume_root(volume: &[u16], path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let mut path_wide = path.as_os_str().encode_wide();
    let Some(path_drive) = path_wide.next() else {
        return false;
    };
    let Some(colon) = path_wide.next() else {
        return false;
    };
    volume.len() == 3
        && colon == u16::from(b':')
        && windows_ascii_upper(volume[0]) == windows_ascii_upper(path_drive)
        && volume[1] == u16::from(b':')
        && matches!(volume[2], value if value == u16::from(b'\\') || value == u16::from(b'/'))
}

#[cfg(windows)]
fn windows_ascii_upper(value: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&value) {
        value - u16::from(b'a') + u16::from(b'A')
    } else {
        value
    }
}

#[cfg(not(windows))]
fn system_windows_volume_kind(_path: &Path) -> WindowsVolumeKind {
    WindowsVolumeKind::Missing
}

pub struct MountpointAllocator<'a> {
    home: PathBuf,
    windows: bool,
    probe: &'a dyn MountpointProbe,
    reserved: HashSet<String>,
}

/// Validate a configured custom mountpoint against the current host.
///
/// The configured value may use `~` to refer to `home`; the returned path is
/// always the expanded value.  Automatic mountpoint values and drive-letter
/// allocation are intentionally outside this API: callers should invoke this
/// only after selecting a custom path.
pub fn preflight_custom_mountpoint(
    configured: &str,
    home: &Path,
) -> Result<PathBuf, MountpointError> {
    let probe = SystemMountpointProbe;
    let path = expand_home(Path::new(configured.trim()), home);
    validate_custom_path(&path, cfg!(windows), &probe, |_| false)?;
    Ok(path)
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
        validate_custom_path(path, self.windows, self.probe, |candidate| {
            self.is_reserved(candidate)
        })
    }

    fn is_reserved(&self, path: &Path) -> bool {
        self.reserved.contains(&path_key(path, self.windows))
    }
}

fn validate_custom_path(
    path: &Path,
    windows: bool,
    probe: &dyn MountpointProbe,
    is_reserved: impl Fn(&Path) -> bool,
) -> Result<(), MountpointError> {
    if !path_is_absolute(path, windows) {
        return Err(MountpointError::NotAbsolute(path.to_owned()));
    }
    if windows && is_windows_unc(path) {
        return Err(MountpointError::WindowsUncPath(path.to_owned()));
    }
    if is_reserved(path) || probe.path_is_mount(path) {
        return Err(MountpointError::AlreadyReserved(path.to_owned()));
    }
    if windows {
        let parent = windows_parent(path);
        if !probe.path_exists(&parent) {
            return Err(MountpointError::ParentMissing(parent));
        }
        if probe.path_exists(path) {
            return Err(MountpointError::WindowsTargetExists(path.to_owned()));
        }
        match probe.windows_volume_kind(&parent) {
            WindowsVolumeKind::Fixed => {}
            WindowsVolumeKind::Missing => {
                return Err(MountpointError::WindowsVolumeMissing(parent));
            }
            kind => {
                return Err(MountpointError::WindowsVolumeUnsupported { path: parent, kind });
            }
        }
    } else if probe.path_exists(path) && !probe.path_is_dir(path) {
        return Err(MountpointError::NotDirectory(path.to_owned()));
    }
    Ok(())
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
        .map(|index| {
            // Preserve the root separator for `C:\\child`; without it the
            // volume probe would receive the drive-relative path `C:`.
            let end = if index == 2 { index + 1 } else { index };
            PathBuf::from(&value[..end])
        })
        .unwrap_or_else(|| path.parent().unwrap_or_else(|| Path::new("")).to_owned())
}

fn is_windows_unc(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with("\\\\") || value.starts_with("//")
}

fn path_is_absolute(path: &Path, windows: bool) -> bool {
    if !windows {
        return path.as_os_str().to_string_lossy().starts_with('/');
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

pub(crate) fn path_key(path: &Path, windows: bool) -> String {
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
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeProbe {
        drives: HashSet<char>,
        existing: HashSet<String>,
        directories: HashSet<String>,
        mounts: HashSet<String>,
        volumes: HashMap<String, WindowsVolumeKind>,
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

        fn windows_volume_kind(&self, path: &Path) -> WindowsVolumeKind {
            self.volumes
                .get(&path_key(path, true))
                .copied()
                .unwrap_or(WindowsVolumeKind::Missing)
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
        probe
            .volumes
            .insert("c:/mounts".into(), WindowsVolumeKind::Fixed);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("C:\\mounts\\alpha")).unwrap(),
            PathBuf::from("C:\\mounts\\alpha")
        );
    }

    #[test]
    fn windows_folder_rejects_existing_target() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("c:/mounts".into());
        probe.existing.insert("c:/mounts/alpha".into());
        probe
            .volumes
            .insert("c:/mounts".into(), WindowsVolumeKind::Fixed);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("C:\\mounts\\alpha")),
            Err(MountpointError::WindowsTargetExists(PathBuf::from(
                "C:\\mounts\\alpha"
            )))
        );
    }

    #[test]
    fn windows_folder_rejects_missing_parent() {
        let probe = FakeProbe::default();
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("C:\\missing\\alpha")),
            Err(MountpointError::ParentMissing(PathBuf::from("C:\\missing")))
        );
    }

    #[test]
    fn custom_mountpoint_rejects_nonabsolute_path() {
        let probe = FakeProbe::default();
        let mut allocator = MountpointAllocator::new(PathBuf::from("/home/me"), false, &probe);

        assert_eq!(
            allocator.resolve(&server("relative/mount")),
            Err(MountpointError::NotAbsolute(PathBuf::from(
                "relative/mount"
            )))
        );
    }

    #[test]
    fn non_windows_custom_mountpoint_rejects_existing_non_directory() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("/tmp/mount".into());
        let mut allocator = MountpointAllocator::new(PathBuf::from("/home/me"), false, &probe);

        assert_eq!(
            allocator.resolve(&server("/tmp/mount")),
            Err(MountpointError::NotDirectory(PathBuf::from("/tmp/mount")))
        );
    }

    #[test]
    fn windows_folder_rejects_remote_backing_volume() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("z:/mounts".into());
        probe
            .volumes
            .insert("z:/mounts".into(), WindowsVolumeKind::Remote);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("Z:\\mounts\\alpha")),
            Err(MountpointError::WindowsVolumeUnsupported {
                path: PathBuf::from("Z:\\mounts"),
                kind: WindowsVolumeKind::Remote,
            })
        );
    }

    #[test]
    fn windows_folder_rejects_missing_backing_volume() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("c:/mounts".into());
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("C:\\mounts\\alpha")),
            Err(MountpointError::WindowsVolumeMissing(PathBuf::from(
                "C:\\mounts"
            )))
        );
    }

    #[test]
    fn windows_folder_rejects_unsupported_backing_volume() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("e:/mounts".into());
        probe
            .volumes
            .insert("e:/mounts".into(), WindowsVolumeKind::Removable);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert!(matches!(
            allocator.resolve(&server("E:\\mounts\\alpha")),
            Err(MountpointError::WindowsVolumeUnsupported {
                kind: WindowsVolumeKind::Removable,
                ..
            })
        ));
    }

    #[test]
    fn windows_folder_rejects_mounted_reparse_and_non_ntfs_backing_paths() {
        for kind in [
            WindowsVolumeKind::Mounted,
            WindowsVolumeKind::Reparse,
            WindowsVolumeKind::NonNtfs,
        ] {
            let mut probe = FakeProbe::default();
            probe.existing.insert("c:/mounts".into());
            probe.volumes.insert("c:/mounts".into(), kind);
            let mut allocator =
                MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

            assert_eq!(
                allocator.resolve(&server("C:\\mounts\\alpha")),
                Err(MountpointError::WindowsVolumeUnsupported {
                    path: PathBuf::from("C:\\mounts"),
                    kind,
                })
            );
        }
    }

    #[test]
    fn windows_folder_rejects_unc_before_parent_or_volume_probe() {
        let mut probe = FakeProbe::default();
        probe.existing.insert("//server/share".into());
        probe
            .volumes
            .insert("//server/share".into(), WindowsVolumeKind::Fixed);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(
            allocator.resolve(&server("\\\\server\\share\\alpha")),
            Err(MountpointError::WindowsUncPath(PathBuf::from(
                "\\\\server\\share\\alpha"
            )))
        );
    }

    #[test]
    fn drive_letter_mountpoints_skip_folder_volume_preflight() {
        let mut probe = FakeProbe::default();
        probe.volumes.insert("z:".into(), WindowsVolumeKind::Remote);
        let mut allocator = MountpointAllocator::new(PathBuf::from("C:/Users/me"), true, &probe);

        assert_eq!(allocator.resolve(&server("Z:")), Ok(PathBuf::from("Z:")));
    }

    #[cfg(windows)]
    #[test]
    fn native_volume_probe_resolves_the_current_windows_volume() {
        let current =
            std::env::current_dir().expect("Windows test process has a current directory");
        assert_ne!(
            system_windows_volume_kind(&current),
            WindowsVolumeKind::Missing
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
