use std::path::Path;

use mountmate_core::ssh::SshPermissionControl;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalProgressState {
    Hidden,
    Indeterminate,
    Normal { completed: u64, total: u64 },
    Paused { completed: u64, total: u64 },
    Error { completed: u64, total: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub progress: Option<(u64, u64)>,
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{0} is not supported on this desktop environment")]
    Unsupported(&'static str),
    #[error("platform integration failed: {0}")]
    Failed(String),
}

pub trait PlatformIntegration: Send + Sync {
    fn show_notification(&self, notification: &Notification) -> Result<(), PlatformError>;
    fn set_global_progress(&self, state: GlobalProgressState) -> Result<(), PlatformError>;
    fn register_file_manager_menu(&self, executable: &Path) -> Result<(), PlatformError>;
    fn unregister_file_manager_menu(&self) -> Result<(), PlatformError>;
}

pub struct Platform;

impl SshPermissionControl for Platform {
    fn restrict_private_path(&self, path: &Path, directory: bool) -> Result<(), String> {
        restrict_private_path(path, directory)
    }
}

#[cfg(unix)]
fn restrict_private_path(path: &Path, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if directory { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
fn restrict_private_path(path: &Path, directory: bool) -> Result<(), String> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation,
        InitializeAcl, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
        SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    fn aligned_buffer(bytes: usize) -> Vec<usize> {
        vec![0; bytes.div_ceil(size_of::<usize>())]
    }

    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let _token = OwnedHandle(token);
    let mut token_bytes = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut token_bytes);
    }
    if token_bytes == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut token_buffer = aligned_buffer(token_bytes as usize);
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            token_buffer.as_mut_ptr().cast(),
            token_bytes,
            &mut token_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let user_sid = unsafe { (*(token_buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let user_sid_bytes = unsafe { GetLengthSid(user_sid) };
    if user_sid_bytes == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let mut system_buffer = aligned_buffer(SECURITY_MAX_SID_SIZE as usize);
    let system_sid: PSID = system_buffer.as_mut_ptr().cast::<c_void>();
    let mut system_sid_bytes = SECURITY_MAX_SID_SIZE;
    if unsafe {
        CreateWellKnownSid(
            WinLocalSystemSid,
            null_mut(),
            system_sid,
            &mut system_sid_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let ace_header = size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>();
    let acl_bytes =
        size_of::<ACL>() + ace_header * 2 + user_sid_bytes as usize + system_sid_bytes as usize;
    let mut acl_buffer = aligned_buffer(acl_bytes);
    let acl = acl_buffer.as_mut_ptr().cast::<ACL>();
    if unsafe { InitializeAcl(acl, acl_bytes as u32, ACL_REVISION) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let inheritance = if directory {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        0
    };
    for sid in [user_sid, system_sid] {
        if unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION, inheritance, FILE_ALL_ACCESS, sid) }
            == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(result as i32).to_string())
    }
}

#[cfg(not(any(unix, windows)))]
fn restrict_private_path(_path: &Path, _directory: bool) -> Result<(), String> {
    Err("private SSH permissions are not supported on this platform".into())
}

impl PlatformIntegration for Platform {
    fn show_notification(&self, _notification: &Notification) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("native notifications"))
    }

    fn set_global_progress(&self, _state: GlobalProgressState) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("taskbar or dock progress"))
    }

    fn register_file_manager_menu(&self, _executable: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("file-manager integration"))
    }

    fn unregister_file_manager_menu(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("file-manager integration"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn private_ssh_permissions_are_exact() {
        let temp = tempdir().unwrap();
        let directory = temp.path().join(".ssh");
        let file = directory.join("config");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&file, "Host alpha\n").unwrap();

        let platform = Platform;
        platform.restrict_private_path(&directory, true).unwrap();
        platform.restrict_private_path(&file, false).unwrap();

        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
