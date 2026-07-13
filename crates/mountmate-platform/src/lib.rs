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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindowHandle(pub isize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub progress: Option<(u64, u64)>,
    pub level: NotificationLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Error,
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
    fn set_global_progress(
        &self,
        window: Option<NativeWindowHandle>,
        state: GlobalProgressState,
    ) -> Result<(), PlatformError>;
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
    fn show_notification(&self, notification: &Notification) -> Result<(), PlatformError> {
        show_notification(notification)
    }

    fn set_global_progress(
        &self,
        window: Option<NativeWindowHandle>,
        state: GlobalProgressState,
    ) -> Result<(), PlatformError> {
        set_global_progress(window, state)
    }

    fn register_file_manager_menu(&self, executable: &Path) -> Result<(), PlatformError> {
        register_file_manager_menu(executable)
    }

    fn unregister_file_manager_menu(&self) -> Result<(), PlatformError> {
        unregister_file_manager_menu()
    }
}

#[cfg(windows)]
const NOTIFICATION_APP_ID: &str = "Stardust.SSHMountMate";

fn show_notification(notification: &Notification) -> Result<(), PlatformError> {
    let mut native = notify_rust::Notification::new();
    native
        .appname(mountmate_core::APP_NAME)
        .summary(&notification.title)
        .body(&notification.body);

    #[cfg(windows)]
    {
        ensure_windows_notification_identity()?;
        let _apartment = WindowsRuntimeApartment::initialize()?;
        native.app_id(NOTIFICATION_APP_ID);
    }
    #[cfg(target_os = "macos")]
    ensure_macos_notification_identity()?;
    #[cfg(not(target_os = "macos"))]
    if notification.level == NotificationLevel::Error {
        native.urgency(notify_rust::Urgency::Critical);
    }

    native
        .show()
        .map(|_| ())
        .map_err(|error| PlatformError::Failed(error.to_string()))
}

#[cfg(windows)]
struct WindowsRuntimeApartment;

#[cfg(windows)]
impl WindowsRuntimeApartment {
    fn initialize() -> Result<Self, PlatformError> {
        use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for WindowsRuntimeApartment {
    fn drop(&mut self) {
        use windows::Win32::System::WinRT::RoUninitialize;

        unsafe {
            RoUninitialize();
        }
    }
}

#[cfg(windows)]
fn ensure_windows_notification_identity() -> Result<(), PlatformError> {
    use std::sync::OnceLock;

    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            let key = windows_registry::CURRENT_USER
                .create(format!(
                    r"Software\Classes\AppUserModelId\{NOTIFICATION_APP_ID}"
                ))
                .map_err(|error| error.to_string())?;
            key.set_string("DisplayName", mountmate_core::APP_NAME)
                .map_err(|error| error.to_string())?;
            key.set_string("IconBackgroundColor", "0")
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .clone()
        .map_err(PlatformError::Failed)
}

#[cfg(target_os = "macos")]
fn ensure_macos_notification_identity() -> Result<(), PlatformError> {
    use std::sync::OnceLock;

    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            notify_rust::set_application("com.stardust.sshmountmate")
                .map_err(|error| error.to_string())
        })
        .clone()
        .map_err(PlatformError::Failed)
}

#[cfg(windows)]
fn set_global_progress(
    window: Option<NativeWindowHandle>,
    state: GlobalProgressState,
) -> Result<(), PlatformError> {
    use std::cell::RefCell;
    use std::ffi::c_void;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{
        ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
        TaskbarList,
    };

    let Some(NativeWindowHandle(window)) = window else {
        return Err(PlatformError::Failed(
            "Windows taskbar progress requires a window handle".into(),
        ));
    };
    if window == 0 {
        return Err(PlatformError::Failed(
            "Windows taskbar progress received an invalid window handle".into(),
        ));
    }

    let window = HWND(window as *mut c_void);
    let (flag, progress) = match state {
        GlobalProgressState::Hidden => (TBPF_NOPROGRESS, None),
        GlobalProgressState::Indeterminate => (TBPF_INDETERMINATE, None),
        GlobalProgressState::Normal { completed, total } => (TBPF_NORMAL, Some((completed, total))),
        GlobalProgressState::Paused { completed, total } => (TBPF_PAUSED, Some((completed, total))),
        GlobalProgressState::Error { completed, total } => (TBPF_ERROR, Some((completed, total))),
    };
    thread_local! {
        static TASKBAR: RefCell<Option<ITaskbarList3>> = const { RefCell::new(None) };
    }
    TASKBAR.with(|taskbar| {
        let mut taskbar = taskbar.borrow_mut();
        if taskbar.is_none() {
            let created: ITaskbarList3 = unsafe {
                CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)
                    .map_err(|error| PlatformError::Failed(error.to_string()))?
            };
            unsafe {
                created
                    .HrInit()
                    .map_err(|error| PlatformError::Failed(error.to_string()))?;
            }
            *taskbar = Some(created);
        }
        let result = unsafe {
            let taskbar = taskbar.as_ref().expect("taskbar object was initialized");
            taskbar.SetProgressState(window, flag).and_then(|_| {
                if let Some((completed, total)) = progress {
                    taskbar.SetProgressValue(window, completed.min(total), total.max(1))
                } else {
                    Ok(())
                }
            })
        };
        if let Err(error) = result {
            *taskbar = None;
            return Err(PlatformError::Failed(error.to_string()));
        }
        Ok(())
    })
}

#[cfg(not(windows))]
fn set_global_progress(
    _window: Option<NativeWindowHandle>,
    _state: GlobalProgressState,
) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("taskbar or dock progress"))
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExplorerMenuEntry {
    key: &'static str,
    label: &'static str,
    action: &'static str,
    placeholder: Option<&'static str>,
}

#[cfg(any(windows, test))]
const EXPLORER_MENU_ENTRIES: [ExplorerMenuEntry; 6] = [
    ExplorerMenuEntry {
        key: r"Directory\Background\shell\SSHMountMate.Refresh",
        label: "Refresh with SSH MountMate",
        action: "--refresh-path",
        placeholder: Some("%V"),
    },
    ExplorerMenuEntry {
        key: r"Directory\shell\SSHMountMate.Refresh",
        label: "Refresh with SSH MountMate",
        action: "--refresh-path",
        placeholder: Some("%1"),
    },
    ExplorerMenuEntry {
        key: r"Drive\shell\SSHMountMate.Refresh",
        label: "Refresh with SSH MountMate",
        action: "--refresh-path",
        placeholder: Some("%1"),
    },
    ExplorerMenuEntry {
        key: r"Directory\Background\shell\SSHMountMate.Transfers",
        label: "Open SSH MountMate transfers",
        action: "--show-transfers",
        placeholder: None,
    },
    ExplorerMenuEntry {
        key: r"Directory\shell\SSHMountMate.Transfers",
        label: "Open SSH MountMate transfers",
        action: "--show-transfers",
        placeholder: None,
    },
    ExplorerMenuEntry {
        key: r"Drive\shell\SSHMountMate.Transfers",
        label: "Open SSH MountMate transfers",
        action: "--show-transfers",
        placeholder: None,
    },
];

#[cfg(any(windows, test))]
fn explorer_command(executable: &Path, entry: ExplorerMenuEntry) -> String {
    let executable = executable.to_string_lossy();
    match entry.placeholder {
        Some(placeholder) => format!(r#""{executable}" {} "{placeholder}\.""#, entry.action),
        None => format!(r#""{executable}" {}"#, entry.action),
    }
}

#[cfg(windows)]
fn register_file_manager_menu(executable: &Path) -> Result<(), PlatformError> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    ensure_windows_notification_identity()?;

    struct OwnedKey(HKEY);
    impl Drop for OwnedKey {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    RegCloseKey(self.0);
                }
            }
        }
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<(), PlatformError> {
        let name = name.map(|name| wide(std::ffi::OsStr::new(name)));
        let value = wide(std::ffi::OsStr::new(value));
        let result = unsafe {
            RegSetValueExW(
                key,
                name.as_ref().map_or(null(), |name| name.as_ptr()),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * size_of::<u16>()) as u32,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ))
        }
    }

    for entry in EXPLORER_MENU_ENTRIES {
        let path = wide(std::ffi::OsStr::new(&format!(
            r"Software\Classes\{}",
            entry.key
        )));
        let mut key = null_mut();
        let result = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if result != 0 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
        let key = OwnedKey(key);
        set_string(key.0, None, entry.label)?;
        set_string(key.0, Some("Icon"), &executable.to_string_lossy())?;

        let command_path = wide(std::ffi::OsStr::new(&format!(
            r"Software\Classes\{}\command",
            entry.key
        )));
        let mut command_key = null_mut();
        let result = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                command_path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut command_key,
                null_mut(),
            )
        };
        if result != 0 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
        let command_key = OwnedKey(command_key);
        set_string(command_key.0, None, &explorer_command(executable, entry))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn register_file_manager_menu(_executable: &Path) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("file-manager integration"))
}

#[cfg(windows)]
fn unregister_file_manager_menu() -> Result<(), PlatformError> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};

    for entry in EXPLORER_MENU_ENTRIES {
        let path: Vec<u16> = std::ffi::OsStr::new(&format!(r"Software\Classes\{}", entry.key))
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        if result != 0 && result != 2 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn unregister_file_manager_menu() -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("file-manager integration"))
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

    #[test]
    fn explorer_refresh_commands_protect_drive_root_quotes() {
        let executable = Path::new(r"C:\Program Files\SSH MountMate\SSHMountMate.exe");
        let commands: Vec<_> = EXPLORER_MENU_ENTRIES
            .iter()
            .copied()
            .filter(|entry| entry.action == "--refresh-path")
            .map(|entry| explorer_command(executable, entry))
            .collect();

        assert_eq!(commands.len(), 3);
        assert!(
            commands
                .iter()
                .any(|command| command.ends_with(r#""%V\.""#))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.ends_with(r#""%1\.""#))
        );
        assert!(
            commands.iter().all(|command| command.starts_with(
                r#""C:\Program Files\SSH MountMate\SSHMountMate.exe" --refresh-path "#
            ))
        );
    }

    #[test]
    fn explorer_transfer_commands_reuse_the_main_executable() {
        let executable = Path::new(r"C:\SSHMountMate.exe");
        let entry = EXPLORER_MENU_ENTRIES
            .iter()
            .copied()
            .find(|entry| entry.action == "--show-transfers")
            .unwrap();

        assert_eq!(
            explorer_command(executable, entry),
            r#""C:\SSHMountMate.exe" --show-transfers"#
        );
    }
}
