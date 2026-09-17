#[cfg(any(windows, test))]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::paths::AppPaths;
use crate::rclone_binary::file_sha256;
use crate::storage::{FileLock, atomic_write};

pub const VERSION: &str = env!("SSH_MOUNTMATE_WINFSP_VERSION");
pub const SHA256: &str = env!("SSH_MOUNTMATE_WINFSP_SHA256");
pub const ATTRIBUTION: &str =
    "WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos";
pub const PROJECT_URL: &str = "https://github.com/winfsp/winfsp";
const EMBEDDED_MSI: &[u8] = include_bytes!(env!("SSH_MOUNTMATE_EMBEDDED_WINFSP_PATH"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    RestartRequired,
    Cancelled,
}

pub fn installer_is_embedded() -> bool {
    cfg!(windows) && !EMBEDDED_MSI.is_empty()
}

/// No network fallback: release builds carry the exact official MSI.
pub fn installer_path(paths: &AppPaths) -> Result<PathBuf, String> {
    if !installer_is_embedded() {
        return Err("This build does not contain the Windows mounting component".into());
    }
    materialize_installer(&paths.data_dir.join("installers"), EMBEDDED_MSI, SHA256)
}

fn materialize_installer(directory: &Path, bytes: &[u8], digest: &str) -> Result<PathBuf, String> {
    if digest.len() != 64 || format!("{:x}", Sha256::digest(bytes)) != digest {
        return Err("WinFsp installer checksum mismatch".into());
    }
    let _lock = FileLock::acquire(&directory.join("winfsp.lock"), Duration::from_secs(30))
        .map_err(|error| error.to_string())?;
    let target = directory.join(format!("winfsp-{digest}.msi"));
    if target.is_file() && file_sha256(&target).map_err(|error| error.to_string())? == digest {
        return Ok(target);
    }
    atomic_write(&target, bytes).map_err(|error| error.to_string())?;
    if file_sha256(&target).map_err(|error| error.to_string())? != digest {
        return Err("Extracted WinFsp installer checksum mismatch".into());
    }
    Ok(target)
}

#[cfg(any(windows, test))]
fn installer_parameters(msi: &Path, log: &Path) -> Result<String, String> {
    let quote = |path: &Path| {
        let path = path.to_string_lossy();
        if path.contains(['"', '\0', '\r', '\n']) {
            return Err("Invalid WinFsp installer path".to_owned());
        }
        Ok(format!("\"{path}\""))
    };
    Ok(format!(
        "/i {} /passive /norestart INSTALLLEVEL=1000 /l*v {}",
        quote(msi)?,
        quote(log)?
    ))
}

#[cfg(any(windows, test))]
fn installer_outcome(code: u32) -> Result<InstallOutcome, String> {
    match code {
        0 => Ok(InstallOutcome::Installed),
        3010 | 1641 => Ok(InstallOutcome::RestartRequired),
        1602 => Ok(InstallOutcome::Cancelled),
        _ => Err(format!("Windows Installer returned error {code}")),
    }
}

#[cfg(not(windows))]
pub fn install(_paths: &AppPaths) -> Result<InstallOutcome, String> {
    Err("WinFsp installation is only available on Windows".into())
}

#[cfg(windows)]
pub fn install(paths: &AppPaths) -> Result<InstallOutcome, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, RPC_E_CHANGED_MODE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    if crate::dependency::mount_dependency_available(crate::MountBackend::Fuse) {
        return Ok(InstallOutcome::Installed);
    }
    // Shell extensions used by elevation expect COM on the calling thread.
    let com_result = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if com_result < 0 && com_result != RPC_E_CHANGED_MODE {
        return Err(format!(
            "Windows installer initialization failed: {com_result:#x}"
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
    let _com = ComGuard(com_result >= 0);
    let msi = installer_path(paths)?;
    // UAC may use a different administrator account. This public installer is
    // readable by administrators while private app configuration stays private.
    allow_elevated_installer_access(&msi, false)?;
    // Keep the verified MSI open without write/delete sharing until msiexec exits.
    let _installer = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&msi)
        .map_err(|error| error.to_string())?;
    if file_sha256(&msi).map_err(|error| error.to_string())? != SHA256 {
        return Err("WinFsp installer checksum mismatch".into());
    }
    fs::create_dir_all(&paths.state_dir).map_err(|error| error.to_string())?;
    let log = paths.state_dir.join("winfsp-install.log");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|error| error.to_string())?;
    allow_elevated_installer_access(&log, true)?;
    let parameters = installer_parameters(&msi, &log)?;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let parameters = wide(&parameters);
    let verb = wide("runas");
    let system_root = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
    let executable = wide(
        &PathBuf::from(system_root)
            .join("System32/msiexec.exe")
            .to_string_lossy(),
    );
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: 1,
        ..Default::default()
    };
    // All pointers remain valid through ShellExecuteExW; OwnedHandle closes the
    // returned process handle. Only the installer is elevated, never the app.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            Ok(InstallOutcome::Cancelled)
        } else {
            Err(error.to_string())
        };
    }
    if info.hProcess.is_null() {
        return Err("No Windows Installer process was returned".into());
    }
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let outcome =
        installer_outcome(code).map_err(|error| format!("{error}; log: {}", log.display()))?;
    if outcome == InstallOutcome::Installed
        && !crate::dependency::mount_dependency_available(crate::MountBackend::Fuse)
    {
        return Err(format!(
            "Mounting component was not detected; log: {}",
            log.display()
        ));
    }
    Ok(outcome)
}

#[cfg(windows)]
fn allow_elevated_installer_access(path: &Path, writable: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };
    // Owner and SYSTEM retain full access. Administrators may read the public
    // MSI, and write only its install log, even when UAC uses another account.
    let acl = if writable {
        "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)"
    } else {
        "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FR;;;BA)"
    };
    let acl: Vec<u16> = acl.encode_utf16().chain(Some(0)).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            acl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let result = if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    };
    unsafe { LocalFree(descriptor) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_cache_is_verified_and_tampering_is_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let payload = b"installer fixture";
        let digest = format!("{:x}", Sha256::digest(payload));
        let path = materialize_installer(temp.path(), payload, &digest).unwrap();
        assert_eq!(path.extension().unwrap(), "msi");
        fs::write(&path, "tampered").unwrap();
        assert_eq!(
            materialize_installer(temp.path(), payload, &digest).unwrap(),
            path
        );
        assert_eq!(fs::read(&path).unwrap(), payload);
        assert!(materialize_installer(temp.path(), b"wrong", &digest).is_err());
    }

    #[test]
    fn installation_handles_spaces_cancellation_and_restart() {
        let parameters = installer_parameters(
            Path::new("C:/User Name/winfsp.msi"),
            Path::new("C:/User Name/install.log"),
        )
        .unwrap();
        assert!(parameters.starts_with("/i \"C:/User Name/winfsp.msi\" /passive /norestart"));
        assert!(parameters.contains("INSTALLLEVEL=1000"));
        assert!(installer_parameters(Path::new("bad\"path"), Path::new("log")).is_err());
        assert_eq!(installer_outcome(0), Ok(InstallOutcome::Installed));
        assert_eq!(installer_outcome(1602), Ok(InstallOutcome::Cancelled));
        assert_eq!(installer_outcome(3010), Ok(InstallOutcome::RestartRequired));
        assert!(installer_outcome(1603).is_err());
    }

    #[test]
    fn embedded_installer_is_windows_only_and_matches_the_pin() {
        if !cfg!(windows) {
            assert!(EMBEDDED_MSI.is_empty());
        }
        if !EMBEDDED_MSI.is_empty() {
            assert_eq!(format!("{:x}", Sha256::digest(EMBEDDED_MSI)), SHA256);
        }
    }
}
