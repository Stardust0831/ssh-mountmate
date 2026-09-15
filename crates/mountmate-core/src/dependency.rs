use std::path::{Path, PathBuf};

use crate::MountBackend;
use crate::paths::AppPaths;
use crate::plink_binary::resolve_plink;
use crate::rclone::MountPlatform;
use crate::rclone_binary::{
    RcloneBinaryError, ResolvedRclone, find_system_executable, resolve_rclone,
};

pub const WINFSP_INSTALL_URL: &str = "https://winfsp.dev/rel/";

fn winfsp_install_command(winget: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(winget);
    command.args([
        "install",
        "--id",
        "WinFsp.WinFsp",
        "--exact",
        "--source",
        "winget",
        "--silent",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--disable-interactivity",
    ]);
    command.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Called only after the user accepts the installation prompt. The installer
/// may still show Windows' elevation prompt; winget handles the signed package.
pub fn install_winfsp_via_winget() -> Result<(), String> {
    if !cfg!(windows) {
        return Err("WinFsp installation is only available on Windows".into());
    }
    if mount_dependency_available(MountBackend::Fuse) {
        return Ok(());
    }
    let winget = find_system_executable("winget.exe")
        .ok_or_else(|| "winget.exe was not found".to_owned())?;
    let output = winfsp_install_command(&winget)
        .output()
        .map_err(|error| format!("Could not start winget: {error}"))?;
    if !output.status.success() {
        return Err(format!("winget exited with {}", output.status));
    }
    if !mount_dependency_available(MountBackend::Fuse) {
        return Err("WinFsp is not available yet; Windows may need to restart".into());
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DependencyStatus {
    pub rclone: Option<ResolvedRclone>,
    pub mount_dependency: &'static str,
    pub mount_dependency_installed: bool,
    pub openssh: Option<PathBuf>,
    pub plink: Option<PathBuf>,
}

impl DependencyStatus {
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.rclone.is_none() {
            missing.push("rclone");
        }
        if !self.mount_dependency_installed {
            missing.push(self.mount_dependency);
        }
        if self.openssh.is_none() {
            missing.push("OpenSSH");
        }
        missing
    }
}

pub fn check_dependencies(
    paths: &AppPaths,
    app_root: &Path,
    selected_backend: MountBackend,
) -> Result<DependencyStatus, RcloneBinaryError> {
    let rclone = resolve_rclone(paths, app_root, None)?;
    let openssh = find_system_executable(if cfg!(windows) { "ssh.exe" } else { "ssh" });
    let plink = resolve_plink(paths, app_root)?.map(|resolved| resolved.path);
    let (mount_dependency, mount_dependency_installed) =
        mount_dependency_status(selected_backend, MountPlatform::current());
    Ok(DependencyStatus {
        rclone,
        mount_dependency,
        mount_dependency_installed,
        openssh,
        plink,
    })
}

pub fn mount_dependency_status(
    selected_backend: MountBackend,
    platform: MountPlatform,
) -> (&'static str, bool) {
    match platform.effective_backend(selected_backend) {
        MountBackend::Nfs => ("rclone built-in NFS", true),
        MountBackend::Fuse => {
            let name = match platform {
                MountPlatform::Windows => "WinFsp",
                MountPlatform::Macos => "macFUSE / FUSE-T",
                MountPlatform::Linux | MountPlatform::Other => "FUSE",
            };
            (name, fuse_dependency_installed())
        }
    }
}

pub fn mount_dependency_available(selected_backend: MountBackend) -> bool {
    mount_dependency_status(selected_backend, MountPlatform::current()).1
}

#[cfg(windows)]
fn fuse_dependency_installed() -> bool {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .map(|root| root.join("WinFsp"))
        .any(|root| winfsp_runtime_installed_at(&root))
}

#[cfg(any(windows, test))]
fn winfsp_runtime_installed_at(root: &Path) -> bool {
    ["winfsp-x64.dll", "winfsp-x86.dll", "winfsp-a64.dll"]
        .into_iter()
        .any(|name| root.join("bin").join(name).is_file())
}

#[cfg(target_os = "macos")]
fn fuse_dependency_installed() -> bool {
    [
        "/Library/Filesystems/macfuse.fs",
        "/Library/Filesystems/osxfuse.fs",
        "/Library/Frameworks/fuse_t.framework",
        "/Library/Application Support/fuse-t/lib/libfuse-t.dylib",
        "/usr/local/lib/libfuse.dylib",
        "/usr/local/lib/libfuse-t.dylib",
        "/usr/local/lib/libfuse3.dylib",
        "/opt/homebrew/lib/libfuse.dylib",
        "/opt/homebrew/lib/libfuse-t.dylib",
        "/opt/homebrew/lib/libfuse3.dylib",
    ]
    .into_iter()
    .any(|candidate| Path::new(candidate).exists())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn fuse_dependency_installed() -> bool {
    Path::new("/dev/fuse").exists()
        && (find_system_executable("fusermount3").is_some()
            || find_system_executable("fusermount").is_some())
}

#[cfg(not(any(unix, windows)))]
fn fuse_dependency_installed() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winget_install_selects_only_the_exact_winfsp_package() {
        let command = winfsp_install_command(Path::new("C:/Program Files/winget.exe"));
        assert_eq!(command.get_program(), "C:/Program Files/winget.exe");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args[0], "install");
        assert!(
            args.windows(2)
                .any(|args| args == ["--id", "WinFsp.WinFsp"])
        );
        assert!(args.windows(2).any(|args| args == ["--source", "winget"]));
        assert!(args.contains(&"--exact"));
        assert!(args.contains(&"--disable-interactivity"));
        assert!(args.contains(&"--accept-package-agreements"));
    }

    #[test]
    fn empty_winfsp_directory_does_not_count_as_installed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("WinFsp");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        assert!(!winfsp_runtime_installed_at(&root));
        std::fs::write(root.join("bin/winfsp-x64.dll"), b"runtime fixture").unwrap();
        assert!(winfsp_runtime_installed_at(&root));
    }

    #[test]
    fn missing_dependencies_have_stable_user_facing_names() {
        let status = DependencyStatus {
            rclone: None,
            mount_dependency: "FUSE",
            mount_dependency_installed: false,
            openssh: None,
            plink: None,
        };
        assert_eq!(status.missing(), vec!["rclone", "FUSE", "OpenSSH"]);
    }

    #[test]
    fn nfs_skips_fuse_only_on_macos() {
        assert_eq!(
            mount_dependency_status(MountBackend::Nfs, MountPlatform::Macos),
            ("rclone built-in NFS", true)
        );
        assert_eq!(
            mount_dependency_status(MountBackend::Nfs, MountPlatform::Windows).0,
            "WinFsp"
        );
        assert_eq!(
            mount_dependency_status(MountBackend::Nfs, MountPlatform::Linux).0,
            "FUSE"
        );
    }
}
