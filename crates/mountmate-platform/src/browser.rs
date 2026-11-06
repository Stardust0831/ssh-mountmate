use crate::PlatformError;

/// Open a previously validated web URI without file-manager path interpretation.
/// Error messages deliberately omit the URI, which may contain a device code.
pub fn open_browser(uri: &str) -> Result<(), PlatformError> {
    if !(uri.starts_with("https://") || uri.starts_with("http://")) || uri.contains('\0') {
        return Err(PlatformError::Failed("invalid browser URI".into()));
    }
    open_native(uri)
}

#[cfg(windows)]
fn open_native(uri: &str) -> Result<(), PlatformError> {
    use windows_sys::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 && initialized != RPC_E_CHANGED_MODE {
        return Err(PlatformError::Failed(
            "browser initialization failed".into(),
        ));
    }
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    let _com = ComGuard(initialized >= 0);
    let verb = "open".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let target = uri.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut operation = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: target.as_ptr(),
        nShow: 1,
        ..Default::default()
    };
    // Both UTF-16 buffers live through this call. NOASYNC completes dispatch
    // before this worker thread releases its COM apartment.
    if unsafe { ShellExecuteExW(&mut operation) } == 0 {
        return Err(PlatformError::Failed(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_native(uri: &str) -> Result<(), PlatformError> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(program)
        .arg(uri)
        .status()
        .map_err(|error| PlatformError::Failed(error.to_string()))?;
    if !status.success() {
        return Err(PlatformError::Failed(format!(
            "browser dispatch failed ({status})"
        )));
    }
    Ok(())
}
