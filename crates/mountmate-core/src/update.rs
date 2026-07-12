use std::env::consts::{ARCH, OS};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(alias = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpdateValidationError {
    #[error("release asset does not contain a trusted SHA-256 digest")]
    MissingDigest,
    #[error("automatic updates only accept HTTPS assets hosted by GitHub")]
    InvalidHost,
}

pub fn platform_name(os: &str) -> &'static str {
    match os {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

pub fn architecture_name(arch: &str) -> &'static str {
    match arch {
        "aarch64" | "arm64" => "arm64",
        _ => "x64",
    }
}

pub fn expected_asset_name() -> String {
    format!(
        "SSHMountMate-{}-{}.zip",
        platform_name(OS),
        architecture_name(ARCH)
    )
}

pub fn verified_sha256(asset: &ReleaseAsset) -> Result<String, UpdateValidationError> {
    let Some(value) = asset.digest.strip_prefix("sha256:") else {
        return Err(UpdateValidationError::MissingDigest);
    };
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UpdateValidationError::MissingDigest);
    }
    Ok(value.to_ascii_lowercase())
}

pub fn validate_asset_url(asset: &ReleaseAsset) -> Result<(), UpdateValidationError> {
    let Ok(url) = Url::parse(&asset.url) else {
        return Err(UpdateValidationError::InvalidHost);
    };
    if url.scheme() != "https" || !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return Err(UpdateValidationError::InvalidHost);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_cover_release_matrix() {
        assert_eq!(platform_name("windows"), "windows");
        assert_eq!(platform_name("macos"), "macos");
        assert_eq!(architecture_name("aarch64"), "arm64");
        assert_eq!(architecture_name("x86_64"), "x64");
    }

    #[test]
    fn digest_and_url_must_be_trusted() {
        let asset = ReleaseAsset {
            name: "SSHMountMate-windows-x64.zip".into(),
            url: "https://github.com/Stardust0831/ssh-mountmate/releases/download/v1/file.zip"
                .into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size: 42,
        };
        assert_eq!(verified_sha256(&asset).unwrap(), "a".repeat(64));
        assert_eq!(validate_asset_url(&asset), Ok(()));
    }
}
