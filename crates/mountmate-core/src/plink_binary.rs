use std::path::{Path, PathBuf};

use crate::paths::AppPaths;
use crate::rclone_binary::{
    RcloneBinaryError, copy_executable, materialize_verified, verify_bundled,
    write_executable_bytes,
};

const EMBEDDED_PLINK: &[u8] = include_bytes!(env!("SSH_MOUNTMATE_EMBEDDED_PLINK_PATH"));
const EMBEDDED_PLINK_SHA256: &str = env!("SSH_MOUNTMATE_EMBEDDED_PLINK_SHA256");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPlink {
    pub path: PathBuf,
    pub sha256: String,
}

pub fn resolve_plink(
    paths: &AppPaths,
    app_root: &Path,
) -> Result<Option<ResolvedPlink>, RcloneBinaryError> {
    if !cfg!(windows) {
        return Ok(None);
    }
    for bundled in bundled_candidates(app_root) {
        if bundled.is_file() {
            let digest = verify_bundled(&bundled)?;
            let path = materialize_verified(paths, "plink", &digest, true, |target| {
                copy_executable(&bundled, target, true)
            })?;
            return Ok(Some(ResolvedPlink {
                path,
                sha256: digest,
            }));
        }
    }
    if EMBEDDED_PLINK.is_empty() {
        return Ok(None);
    }
    let digest = EMBEDDED_PLINK_SHA256;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RcloneBinaryError::InvalidManifest(PathBuf::from(
            "embedded:plink",
        )));
    }
    let path = materialize_verified(paths, "plink", digest, true, |target| {
        write_executable_bytes(target, EMBEDDED_PLINK, true)
    })?;
    Ok(Some(ResolvedPlink {
        path,
        sha256: digest.into(),
    }))
}

fn bundled_candidates(app_root: &Path) -> [PathBuf; 3] {
    [
        app_root.join("bin/plink.exe"),
        app_root.join("resources/bin/plink.exe"),
        app_root.join("Resources/bin/plink.exe"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn bundled_candidates_are_scoped_to_application_resources() {
        let candidates = bundled_candidates(Path::new("app"));
        assert_eq!(candidates[0], PathBuf::from("app/bin/plink.exe"));
        assert_eq!(candidates[2], PathBuf::from("app/Resources/bin/plink.exe"));
    }

    #[cfg(not(windows))]
    #[test]
    fn plink_is_never_resolved_on_non_windows_platforms() {
        let paths = AppPaths {
            config_dir: PathBuf::from("config"),
            cache_dir: PathBuf::from("cache"),
            state_dir: PathBuf::from("state"),
            data_dir: PathBuf::from("data"),
        };
        assert_eq!(resolve_plink(&paths, Path::new("app")).unwrap(), None);
    }

    #[test]
    fn compiled_plink_payload_matches_its_build_time_digest() {
        if EMBEDDED_PLINK.is_empty() {
            assert!(EMBEDDED_PLINK_SHA256.is_empty());
        } else {
            assert_eq!(
                format!("{:x}", Sha256::digest(EMBEDDED_PLINK)),
                EMBEDDED_PLINK_SHA256
            );
        }
    }
}
