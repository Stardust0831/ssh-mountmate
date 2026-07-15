use std::path::{Path, PathBuf};

use crate::MountBackend;
use crate::paths::AppPaths;
use crate::rclone::MountPlatform;
use crate::rclone_binary::{
    RcloneBinaryError, ResolvedRclone, find_system_executable, resolve_rclone,
};

#[derive(Debug, Clone)]
pub struct DependencyStatus {
    pub rclone: Option<ResolvedRclone>,
    pub mount_dependency: &'static str,
    pub mount_dependency_installed: bool,
    pub openssh: Option<PathBuf>,
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
    let (mount_dependency, mount_dependency_installed) =
        mount_dependency_status(selected_backend, MountPlatform::current());
    Ok(DependencyStatus {
        rclone,
        mount_dependency,
        mount_dependency_installed,
        openssh,
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

#[cfg(windows)]
fn fuse_dependency_installed() -> bool {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .map(|root| root.join("WinFsp"))
        .any(|root| root.is_dir())
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
    fn missing_dependencies_have_stable_user_facing_names() {
        let status = DependencyStatus {
            rclone: None,
            mount_dependency: "FUSE",
            mount_dependency_installed: false,
            openssh: None,
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
