use std::path::Path;

#[cfg(windows)]
use mountmate_core::installed::{
    InstallPolicyError, InstalledInstallRecord, enforce_no_downgrade, validate_installed_identity,
};
use mountmate_core::installed::{InstalledEditionIdentity, enforce_uninstall_preflight};
use mountmate_core::ssh::SshPermissionControl;
use thiserror::Error;

pub mod navigation;
pub use navigation::{NavigationObserver, notify_shell_updated_dir, start_navigation_observer};

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
    fn set_login_startup(&self, executable: &Path, enabled: bool) -> Result<(), PlatformError>;
    /// Return the installed identity only when HKCU's marker and the canonical
    /// fixed executable path both validate. Portable copies return `None`.
    fn installed_edition_identity(
        &self,
        current_executable: &Path,
    ) -> Result<Option<InstalledEditionIdentity>, PlatformError> {
        let _ = current_executable;
        Err(PlatformError::Unsupported("installed-edition identity"))
    }
    /// Enforce the no-implicit-downgrade installer policy against the HKCU
    /// marker. Missing markers are accepted for first-time installation.
    fn enforce_installed_version(&self, requested_version: &str) -> Result<(), PlatformError> {
        let _ = requested_version;
        Err(PlatformError::Unsupported(
            "installed-edition version policy",
        ))
    }
    /// Hook for the app's future uninstall preflight command. Inno Setup uses
    /// a non-zero result to block removal while mounts are active.
    fn uninstall_preflight(&self, active_mounts: bool) -> Result<(), PlatformError> {
        enforce_uninstall_preflight(active_mounts)
            .map_err(|error| PlatformError::Failed(error.to_string()))
    }
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

    fn set_login_startup(&self, executable: &Path, enabled: bool) -> Result<(), PlatformError> {
        set_login_startup(executable, enabled)
    }

    fn installed_edition_identity(
        &self,
        current_executable: &Path,
    ) -> Result<Option<InstalledEditionIdentity>, PlatformError> {
        installed_edition_identity(current_executable)
    }

    fn enforce_installed_version(&self, requested_version: &str) -> Result<(), PlatformError> {
        enforce_installed_version(requested_version)
    }

    fn uninstall_preflight(&self, active_mounts: bool) -> Result<(), PlatformError> {
        enforce_uninstall_preflight(active_mounts)
            .map_err(|error| PlatformError::Failed(error.to_string()))
    }
}

fn installed_edition_identity(
    current_executable: &Path,
) -> Result<Option<InstalledEditionIdentity>, PlatformError> {
    #[cfg(windows)]
    {
        let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
            return Ok(None);
        };
        let Some(record) = read_windows_install_record()? else {
            return Ok(None);
        };
        return validate_installed_identity(
            &record,
            current_executable,
            Path::new(&local_app_data),
        )
        .map(Some)
        .map_err(|error| PlatformError::Failed(error.to_string()));
    }
    #[cfg(not(windows))]
    {
        let _ = current_executable;
        Ok(None)
    }
}

fn enforce_installed_version(requested_version: &str) -> Result<(), PlatformError> {
    #[cfg(windows)]
    {
        let existing = read_windows_install_record()?
            .map(|record| record.version)
            .filter(|version| !version.is_empty());
        return enforce_no_downgrade(existing.as_deref(), requested_version).map_err(|error| {
            match error {
                InstallPolicyError::DowngradeBlocked { .. }
                | InstallPolicyError::InvalidExistingVersion(_)
                | InstallPolicyError::InvalidRequestedVersion(_) => {
                    PlatformError::Failed(error.to_string())
                }
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = requested_version;
        Ok(())
    }
}

#[cfg(windows)]
fn read_windows_install_record() -> Result<Option<InstalledInstallRecord>, PlatformError> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW,
    };

    struct OwnedKey(HKEY);
    impl Drop for OwnedKey {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { RegCloseKey(self.0) };
            }
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }
    fn query_string(key: HKEY, name: &str) -> Result<Option<String>, PlatformError> {
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = 0;
        let result = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null_mut(),
                &mut kind,
                null_mut(),
                &mut bytes,
            )
        };
        if result == 2 {
            return Ok(None);
        }
        if result != 0 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(PlatformError::Failed(format!(
                "install marker value {name:?} is not a string"
            )));
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(size_of::<u16>())];
        let mut capacity = bytes;
        let result = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null_mut(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut capacity,
            )
        };
        if result != 0 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        Ok(Some(String::from_utf16_lossy(&buffer)))
    }
    fn query_dword(key: HKEY, name: &str) -> Result<Option<u32>, PlatformError> {
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = size_of::<u32>() as u32;
        let mut value = 0u32;
        let result = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null_mut(),
                &mut kind,
                (&mut value as *mut u32).cast(),
                &mut bytes,
            )
        };
        if result == 2 {
            return Ok(None);
        }
        if result != 0 {
            return Err(PlatformError::Failed(
                std::io::Error::from_raw_os_error(result as i32).to_string(),
            ));
        }
        if kind != REG_DWORD || bytes != size_of::<u32>() as u32 {
            return Err(PlatformError::Failed(format!(
                "install marker value {name:?} is not a DWORD"
            )));
        }
        Ok(Some(value))
    }

    let path = wide(mountmate_core::installed::WINDOWS_INSTALL_RECORD_KEY);
    let mut key = null_mut();
    let result = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
    if result == 2 {
        return Ok(None);
    }
    if result != 0 {
        return Err(PlatformError::Failed(
            std::io::Error::from_raw_os_error(result as i32).to_string(),
        ));
    }
    let key = OwnedKey(key);
    let Some(schema_version) = query_dword(key.0, "SchemaVersion")? else {
        return Ok(None);
    };
    let Some(version) = query_string(key.0, "Version")? else {
        return Ok(None);
    };
    let Some(install_root) = query_string(key.0, "InstallRoot")? else {
        return Ok(None);
    };
    let Some(executable_path) = query_string(key.0, "ExecutablePath")? else {
        return Ok(None);
    };
    let aumid = query_string(key.0, "Aumid")?.unwrap_or_default();
    let architecture = query_string(key.0, "Architecture")?.unwrap_or_default();
    Ok(Some(InstalledInstallRecord {
        schema_version,
        version,
        install_root: install_root.into(),
        executable_path: executable_path.into(),
        aumid,
        architecture,
    }))
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
            notify_rust::set_application("io.github.stardust0831.ssh-mountmate")
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

#[cfg(target_os = "macos")]
fn set_global_progress(
    _window: Option<NativeWindowHandle>,
    state: GlobalProgressState,
) -> Result<(), PlatformError> {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSImageScaling, NSImageView, NSProgressIndicator, NSProgressIndicatorStyle,
        NSView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    thread_local! {
        static DOCK_PROGRESS: RefCell<Option<Retained<NSProgressIndicator>>> = const {
            RefCell::new(None)
        };
    }

    let mtm = MainThreadMarker::new().ok_or_else(|| {
        PlatformError::Failed("macOS Dock progress must be updated on the main thread".into())
    })?;
    let application = NSApplication::sharedApplication(mtm);
    let dock_tile = application.dockTile();
    DOCK_PROGRESS.with(|stored| {
        let mut stored = stored.borrow_mut();
        if state == GlobalProgressState::Hidden {
            if let Some(indicator) = stored.take() {
                indicator.removeFromSuperview();
            }
            dock_tile.setBadgeLabel(None);
            dock_tile.display();
            return Ok(());
        }

        if stored.is_none() {
            let size = dock_tile.size();
            let content: Retained<NSView> = if let Some(content) = dock_tile.contentView(mtm) {
                content
            } else {
                let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
                let image_view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
                if let Some(icon) = application.applicationIconImage() {
                    image_view.setImage(Some(&icon));
                }
                image_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                let content = image_view.into_super().into_super();
                dock_tile.setContentView(Some(&content));
                content
            };
            let margin = (size.width * 0.08).max(4.0);
            let height = (size.height * 0.13).clamp(8.0, 18.0);
            let frame = NSRect::new(
                NSPoint::new(margin, margin),
                NSSize::new((size.width - margin * 2.0).max(1.0), height),
            );
            let indicator =
                NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), frame);
            indicator.setStyle(NSProgressIndicatorStyle::Bar);
            indicator.setMinValue(0.0);
            indicator.setMaxValue(1.0);
            indicator.setDisplayedWhenStopped(true);
            content.addSubview(&indicator);
            *stored = Some(indicator);
        }

        let indicator = stored
            .as_ref()
            .expect("Dock progress indicator was initialized");
        match state {
            GlobalProgressState::Indeterminate => {
                indicator.setIndeterminate(true);
                unsafe {
                    indicator.setUsesThreadedAnimation(true);
                    indicator.startAnimation(None);
                }
            }
            GlobalProgressState::Normal { completed, total }
            | GlobalProgressState::Paused { completed, total }
            | GlobalProgressState::Error { completed, total } => {
                unsafe {
                    indicator.stopAnimation(None);
                }
                indicator.setIndeterminate(false);
                indicator.setDoubleValue(completed.min(total) as f64 / total.max(1) as f64);
            }
            GlobalProgressState::Hidden => unreachable!("hidden progress returned above"),
        }

        let badge = match state {
            GlobalProgressState::Paused { .. } => Some(NSString::from_str("Ⅱ")),
            GlobalProgressState::Error { .. } => Some(NSString::from_str("!")),
            _ => None,
        };
        dock_tile.setBadgeLabel(badge.as_deref());
        dock_tile.display();
        Ok(())
    })
}

#[cfg(all(not(windows), not(target_os = "macos")))]
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

#[cfg(target_os = "linux")]
fn register_file_manager_menu(executable: &Path) -> Result<(), PlatformError> {
    use std::os::unix::fs::PermissionsExt;

    let home = home_directory()?;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    for entry in linux_file_manager_entries(&data_home, executable)? {
        mountmate_core::storage::atomic_write(&entry.path, entry.content.as_bytes())
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        if entry.executable {
            std::fs::set_permissions(&entry.path, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| PlatformError::Failed(error.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn register_file_manager_menu(executable: &Path) -> Result<(), PlatformError> {
    let services = home_directory()?.join("Library/Services");
    std::fs::create_dir_all(&services).map_err(|error| PlatformError::Failed(error.to_string()))?;
    for workflow in finder_workflows(&services, executable)? {
        let staging = workflow
            .path
            .with_extension(format!("workflow.mountmate-{}.tmp", std::process::id()));
        remove_workflow_if_present(&staging)?;
        std::fs::create_dir_all(staging.join("Contents"))
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        let mut content = Vec::new();
        plist::to_writer_xml(&mut content, &workflow.document)
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        mountmate_core::storage::atomic_write(&staging.join("Contents/document.wflow"), &content)
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        install_directory_atomically(&staging, &workflow.path)?;
    }
    Ok(())
}

#[cfg(all(not(windows), not(any(target_os = "linux", target_os = "macos"))))]
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

#[cfg(target_os = "linux")]
fn unregister_file_manager_menu() -> Result<(), PlatformError> {
    let home = home_directory()?;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    for entry in linux_file_manager_entries(&data_home, Path::new("/unused"))? {
        remove_if_present(&entry.path)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn unregister_file_manager_menu() -> Result<(), PlatformError> {
    let services = home_directory()?.join("Library/Services");
    for name in FINDER_WORKFLOW_NAMES {
        remove_workflow_if_present(&services.join(name))?;
    }
    Ok(())
}

#[cfg(all(not(windows), not(any(target_os = "linux", target_os = "macos"))))]
fn unregister_file_manager_menu() -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("file-manager integration"))
}

#[cfg(target_os = "macos")]
const FINDER_WORKFLOW_NAMES: [&str; 2] = [
    "SSH MountMate - Refresh.workflow",
    "SSH MountMate - Transfers.workflow",
];

#[cfg(target_os = "macos")]
struct FinderWorkflow {
    path: std::path::PathBuf,
    document: plist::Value,
}

#[cfg(target_os = "macos")]
fn finder_workflows(
    services: &Path,
    executable: &Path,
) -> Result<Vec<FinderWorkflow>, PlatformError> {
    let executable = executable
        .to_str()
        .ok_or_else(|| PlatformError::Failed("Finder executable path is not UTF-8".into()))?;
    let executable = shell_single_quote(executable);
    Ok(vec![
        FinderWorkflow {
            path: services.join(FINDER_WORKFLOW_NAMES[0]),
            document: finder_workflow_document(
                &format!("exec {executable} --refresh-path \"$1\""),
                "7A5E99A1-E3B8-4E98-A066-7467D77EA181",
            ),
        },
        FinderWorkflow {
            path: services.join(FINDER_WORKFLOW_NAMES[1]),
            document: finder_workflow_document(
                &format!("exec {executable} --show-transfers"),
                "DD970A96-6879-4267-BE24-5D7751D9343D",
            ),
        },
    ])
}

#[cfg(target_os = "macos")]
fn finder_workflow_document(command: &str, uuid: &str) -> plist::Value {
    use plist::{Dictionary, Value};

    fn string(value: &str) -> Value {
        Value::String(value.into())
    }

    let mut accepts = Dictionary::new();
    accepts.insert("Container".into(), string("List"));
    accepts.insert("Optional".into(), Value::Boolean(true));
    accepts.insert(
        "Types".into(),
        Value::Array(vec![string("com.apple.cocoa.path")]),
    );

    let mut parameters = Dictionary::new();
    parameters.insert("COMMAND_STRING".into(), string(command));
    parameters.insert("CheckedForUserDefaultShell".into(), Value::Boolean(true));
    parameters.insert("inputMethod".into(), Value::Integer(1.into()));
    parameters.insert("shell".into(), string("/bin/zsh"));
    parameters.insert("source".into(), string(""));
    parameters.insert(
        "SUBSTITUTE_VARIABLES_IN_COMMAND".into(),
        Value::Boolean(false),
    );

    let mut action = Dictionary::new();
    action.insert("AMAccepts".into(), Value::Dictionary(accepts.clone()));
    action.insert("AMActionVersion".into(), string("1.1.1"));
    action.insert("AMApplication".into(), Value::Array(vec![string("Finder")]));
    action.insert("AMProvides".into(), Value::Dictionary(accepts));
    action.insert(
        "ActionBundlePath".into(),
        string("/System/Library/Automator/Run Shell Script.action"),
    );
    action.insert("ActionName".into(), string("Run Shell Script"));
    action.insert("ActionParameters".into(), Value::Dictionary(parameters));
    action.insert(
        "BundleIdentifier".into(),
        string("com.apple.RunShellScript"),
    );
    action.insert("Class Name".into(), string("RunShellScriptAction"));
    action.insert("InputUUID".into(), string(uuid));
    action.insert("OutputUUID".into(), string(uuid));
    action.insert("UUID".into(), string(uuid));

    let mut wrapped_action = Dictionary::new();
    wrapped_action.insert("action".into(), Value::Dictionary(action));

    let mut metadata = Dictionary::new();
    metadata.insert("applicationBundleID".into(), string("com.apple.finder"));
    metadata.insert(
        "applicationPath".into(),
        string("/System/Library/CoreServices/Finder.app"),
    );
    metadata.insert(
        "serviceApplicationBundleID".into(),
        string("com.apple.finder"),
    );
    metadata.insert(
        "serviceApplicationPath".into(),
        string("/System/Library/CoreServices/Finder.app"),
    );
    metadata.insert(
        "serviceInputTypeIdentifier".into(),
        string("com.apple.Automator.fileSystemObject"),
    );
    metadata.insert(
        "serviceOutputTypeIdentifier".into(),
        string("com.apple.Automator.nothing"),
    );
    metadata.insert("serviceProcessesInput".into(), Value::Boolean(false));
    metadata.insert(
        "workflowTypeIdentifier".into(),
        string("com.apple.Automator.servicesMenu"),
    );

    let mut document = Dictionary::new();
    document.insert("AMApplicationBuild".into(), string("509"));
    document.insert("AMApplicationVersion".into(), string("2.10"));
    document.insert("AMDocumentVersion".into(), string("2"));
    document.insert(
        "actions".into(),
        Value::Array(vec![Value::Dictionary(wrapped_action)]),
    );
    document.insert("connectors".into(), Value::Dictionary(Dictionary::new()));
    document.insert("workflowMetaData".into(), Value::Dictionary(metadata));
    Value::Dictionary(document)
}

#[cfg(any(target_os = "linux", test))]
struct LinuxFileManagerEntry {
    path: std::path::PathBuf,
    content: String,
    executable: bool,
}

#[cfg(any(target_os = "linux", test))]
fn linux_file_manager_entries(
    data_home: &Path,
    executable: &Path,
) -> Result<Vec<LinuxFileManagerEntry>, PlatformError> {
    let executable = executable
        .to_str()
        .ok_or_else(|| PlatformError::Failed("file-manager executable path is not UTF-8".into()))?;
    let shell_executable = shell_single_quote(executable);
    let desktop_executable = desktop_exec_argument(executable);
    let refresh_script = format!(
        "#!/bin/sh\nset -eu\nselected=${{NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-${{NEMO_SCRIPT_SELECTED_FILE_PATHS:-}}}}\nif [ -n \"$selected\" ]; then\n  path=$(printf '%s\\n' \"$selected\" | head -n 1)\nelse\n  path=$PWD\nfi\nexec {shell_executable} --refresh-path \"$path\"\n"
    );
    let transfers_script =
        format!("#!/bin/sh\nset -eu\nexec {shell_executable} --show-transfers\n");
    let nemo_refresh = format!(
        "[Nemo Action]\nName=Refresh with SSH MountMate\nComment=Refresh the selected mounted directory\nExec={desktop_executable} --refresh-path %P\nSelection=any\nExtensions=dir;\n"
    );
    let nemo_transfers = format!(
        "[Nemo Action]\nName=Open SSH MountMate transfers\nExec={desktop_executable} --show-transfers\nSelection=any\nExtensions=dir;\n"
    );
    let dolphin = format!(
        "[Desktop Entry]\nType=Service\nMimeType=inode/directory;\nActions=SSHMountMateRefresh;SSHMountMateTransfers;\nX-KDE-ServiceTypes=KonqPopupMenu/Plugin\n\n[Desktop Action SSHMountMateRefresh]\nName=Refresh with SSH MountMate\nExec={desktop_executable} --refresh-path %f\n\n[Desktop Action SSHMountMateTransfers]\nName=Open SSH MountMate transfers\nExec={desktop_executable} --show-transfers\n"
    );
    let mut entries = Vec::new();
    for manager in ["nautilus", "nemo"] {
        let scripts = data_home.join(manager).join("scripts");
        entries.push(LinuxFileManagerEntry {
            path: scripts.join("SSH MountMate - Refresh"),
            content: refresh_script.clone(),
            executable: true,
        });
        entries.push(LinuxFileManagerEntry {
            path: scripts.join("SSH MountMate - Transfers"),
            content: transfers_script.clone(),
            executable: true,
        });
    }
    entries.push(LinuxFileManagerEntry {
        path: data_home.join("nemo/actions/ssh-mountmate-refresh.nemo_action"),
        content: nemo_refresh,
        executable: false,
    });
    entries.push(LinuxFileManagerEntry {
        path: data_home.join("nemo/actions/ssh-mountmate-transfers.nemo_action"),
        content: nemo_transfers,
        executable: false,
    });
    for directory in ["kio/servicemenus", "kservices5/ServiceMenus"] {
        entries.push(LinuxFileManagerEntry {
            path: data_home.join(directory).join("ssh-mountmate.desktop"),
            content: dolphin.clone(),
            executable: false,
        });
    }
    Ok(entries)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn set_login_startup(executable: &Path, enabled: bool) -> Result<(), PlatformError> {
    const VALUE_NAME: &str = "SSHMountMate";

    let key = windows_registry::CURRENT_USER
        .create(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .map_err(|error| PlatformError::Failed(error.to_string()))?;
    if enabled {
        key.set_string(VALUE_NAME, &windows_startup_command(executable))
            .map_err(|error| PlatformError::Failed(error.to_string()))
    } else {
        match key.remove_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(error) if error.code().0 as u32 == 0x8007_0002 => Ok(()),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }
}

#[cfg(any(windows, test))]
fn windows_startup_command(executable: &Path) -> String {
    format!(r#""{}" --mount-startup"#, executable.to_string_lossy())
}

#[cfg(target_os = "macos")]
fn set_login_startup(executable: &Path, enabled: bool) -> Result<(), PlatformError> {
    use plist::{Dictionary, Value};

    let home = home_directory()?;
    let path = home
        .join("Library/LaunchAgents")
        .join("io.github.stardust0831.ssh-mountmate.plist");
    if !enabled {
        return remove_if_present(&path).map_err(|error| {
            PlatformError::Failed(format!(
                "could not remove login startup file {}: {error}",
                path.display()
            ))
        });
    }
    let executable = executable
        .to_str()
        .ok_or_else(|| PlatformError::Failed("startup executable path is not UTF-8".into()))?;
    let mut dictionary = Dictionary::new();
    dictionary.insert(
        "Label".into(),
        Value::String("io.github.stardust0831.ssh-mountmate".into()),
    );
    dictionary.insert(
        "ProgramArguments".into(),
        Value::Array(vec![
            Value::String(executable.into()),
            Value::String("--mount-startup".into()),
        ]),
    );
    dictionary.insert("RunAtLoad".into(), Value::Boolean(true));
    dictionary.insert("ProcessType".into(), Value::String("Background".into()));
    let mut content = Vec::new();
    plist::to_writer_xml(&mut content, &Value::Dictionary(dictionary))
        .map_err(|error| PlatformError::Failed(error.to_string()))?;
    mountmate_core::storage::atomic_write(&path, &content).map_err(|error| {
        PlatformError::Failed(format!(
            "could not write login startup file {}: {error}",
            path.display()
        ))
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn set_login_startup(executable: &Path, enabled: bool) -> Result<(), PlatformError> {
    let home = home_directory()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let path = config.join("autostart/ssh-mountmate.desktop");
    if !enabled {
        return remove_if_present(&path);
    }
    let executable = executable
        .to_str()
        .ok_or_else(|| PlatformError::Failed("startup executable path is not UTF-8".into()))?;
    let command = desktop_exec_argument(executable);
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=SSH MountMate\nExec={command} --mount-startup\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    );
    mountmate_core::storage::atomic_write(&path, content.as_bytes())
        .map_err(|error| PlatformError::Failed(error.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn set_login_startup(_executable: &Path, _enabled: bool) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("login startup"))
}

#[cfg(unix)]
fn home_directory() -> Result<std::path::PathBuf, PlatformError> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| PlatformError::Failed("could not locate the user home directory".into()))
}

#[cfg(unix)]
fn remove_if_present(path: &Path) -> Result<(), PlatformError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PlatformError::Failed(error.to_string())),
    }
}

#[cfg(target_os = "macos")]
fn remove_workflow_if_present(path: &Path) -> Result<(), PlatformError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PlatformError::Failed(error.to_string())),
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| PlatformError::Failed(error.to_string()))
}

#[cfg(target_os = "macos")]
fn workflow_exists(path: &Path) -> Result<bool, PlatformError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(PlatformError::Failed(error.to_string())),
    }
}

#[cfg(target_os = "macos")]
fn install_directory_atomically(staging: &Path, destination: &Path) -> Result<(), PlatformError> {
    let backup =
        destination.with_extension(format!("workflow.mountmate-{}.backup", std::process::id()));
    remove_workflow_if_present(&backup)?;
    let had_destination = workflow_exists(destination)?;
    if had_destination {
        std::fs::rename(destination, &backup)
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
    }
    if let Err(error) = std::fs::rename(staging, destination) {
        if had_destination {
            let _ = std::fs::rename(&backup, destination);
        }
        return Err(PlatformError::Failed(error.to_string()));
    }
    remove_workflow_if_present(&backup)
}

#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn desktop_exec_argument(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    format!("\"{escaped}\"")
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

    #[test]
    fn startup_commands_quote_executable_paths_without_a_shell() {
        let executable = Path::new(r"C:\Program Files\SSH MountMate\SSHMountMate.exe");
        assert_eq!(
            windows_startup_command(executable),
            r#""C:\Program Files\SSH MountMate\SSHMountMate.exe" --mount-startup"#
        );
        assert_eq!(
            desktop_exec_argument("/home/user/SSH MountMate/bin/$current"),
            r#""/home/user/SSH MountMate/bin/\$current""#
        );
        assert_eq!(
            desktop_exec_argument("/opt/100%free/SSHMountMate"),
            r#""/opt/100%%free/SSHMountMate""#
        );
    }

    #[test]
    fn linux_file_manager_entries_cover_major_desktops_and_quote_paths() {
        let data = Path::new("/home/user/.local/share");
        let executable = Path::new("/opt/SSH MountMate/user's/SSHMountMate");
        let entries = linux_file_manager_entries(data, executable).unwrap();

        assert_eq!(entries.len(), 8);
        assert!(entries.iter().any(|entry| {
            entry
                .path
                .ends_with("nautilus/scripts/SSH MountMate - Refresh")
                && entry.executable
                && entry
                    .content
                    .contains("exec '/opt/SSH MountMate/user'\"'\"'s/SSHMountMate' --refresh-path")
        }));
        assert!(entries.iter().any(|entry| {
            entry
                .path
                .ends_with("kio/servicemenus/ssh-mountmate.desktop")
                && entry
                    .content
                    .contains("Exec=\"/opt/SSH MountMate/user's/SSHMountMate\" --refresh-path %f")
        }));
        assert!(entries.iter().any(|entry| {
            entry
                .path
                .ends_with("nemo/actions/ssh-mountmate-refresh.nemo_action")
        }));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    use tempfile::tempdir;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CreateWellKnownSid,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetLengthSid,
        GetSecurityDescriptorControl, GetTokenInformation, PSID, SE_DACL_PROTECTED,
        SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::restrict_private_path;

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    fn aligned_buffer(bytes: usize) -> Vec<usize> {
        vec![0; bytes.div_ceil(size_of::<usize>())]
    }

    #[test]
    fn private_windows_acl_is_protected_and_only_allows_user_and_system() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("managed-key");
        std::fs::write(&path, b"private key").unwrap();
        restrict_private_path(&path, false).unwrap();

        let mut token = null_mut();
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0
        );
        let mut token_bytes = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut token_bytes);
        }
        let mut token_buffer = aligned_buffer(token_bytes as usize);
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    token_buffer.as_mut_ptr().cast(),
                    token_bytes,
                    &mut token_bytes,
                )
            },
            0
        );
        unsafe { CloseHandle(token) };
        let user_sid = unsafe { (*(token_buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        assert_ne!(unsafe { GetLengthSid(user_sid) }, 0);

        let mut system_buffer = aligned_buffer(SECURITY_MAX_SID_SIZE as usize);
        let system_sid: PSID = system_buffer.as_mut_ptr().cast::<c_void>();
        let mut system_bytes = SECURITY_MAX_SID_SIZE;
        assert_ne!(
            unsafe {
                CreateWellKnownSid(WinLocalSystemSid, null_mut(), system_sid, &mut system_bytes)
            },
            0
        );

        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor = null_mut();
        assert_eq!(
            unsafe {
                GetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        assert!(!dacl.is_null());
        assert!(!descriptor.is_null());

        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);

        let mut information: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        assert_eq!(information.AceCount, 2);
        let mut found_user = false;
        let mut found_system = false;
        for index in 0..information.AceCount {
            let mut raw_ace = null_mut();
            assert_ne!(unsafe { GetAce(dacl, index, &mut raw_ace) }, 0);
            let ace = unsafe { &*(raw_ace.cast::<ACCESS_ALLOWED_ACE>()) };
            assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);
            let sid = (&ace.SidStart as *const u32).cast_mut().cast::<c_void>();
            found_user |= unsafe { EqualSid(sid, user_sid) } != 0;
            found_system |= unsafe { EqualSid(sid, system_sid) } != 0;
        }
        assert!(found_user);
        assert!(found_system);
        unsafe {
            LocalFree(descriptor);
        }
    }
}
