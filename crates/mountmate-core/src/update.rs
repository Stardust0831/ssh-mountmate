use std::collections::HashSet;
use std::env::consts::{ARCH, OS};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::redirect::Policy;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use zip::ZipArchive;

use crate::update_manifest::{
    MANIFEST_ASSET_NAME, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, ManifestAsset, PublishedAsset,
    ReleaseChannel, SIGNATURE_ASSET_NAME, UpdateTrustError, embedded_trusted_keys,
    verify_release_manifest,
};

const REPOSITORY: &str = "Stardust0831/ssh-mountmate";
const RELEASES_API: &str =
    "https://api.github.com/repos/Stardust0831/ssh-mountmate/releases?per_page=20";
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Stardust0831/ssh-mountmate/releases/latest";
const LATEST_RELEASE_PAGE: &str = "https://github.com/Stardust0831/ssh-mountmate/releases/latest";
const DEFAULT_DOWNLOAD_LIMIT: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 20_000;
const MAX_EXTRACTED_SIZE: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedUpdateAsset {
    name: String,
    url: String,
    size: u64,
    sha256: String,
    version: String,
    channel: ReleaseChannel,
    key_id: String,
}

impl VerifiedUpdateAsset {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    fn from_verified(
        published: &ReleaseAsset,
        signed: &ManifestAsset,
        version: String,
        channel: ReleaseChannel,
        key_id: String,
    ) -> Self {
        Self {
            name: published.name.clone(),
            url: published.url.clone(),
            size: signed.size,
            sha256: signed.sha256.clone(),
            version,
            channel,
            key_id,
        }
    }

    #[cfg(test)]
    fn for_test(payload: &[u8], name: &str) -> Self {
        Self {
            name: name.into(),
            url: format!("https://github.com/{REPOSITORY}/releases/download/v1/{name}"),
            size: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
            version: "1.0.0".into(),
            channel: ReleaseChannel::Stable,
            key_id: "ed25519-0000000000000000".into(),
        }
    }
}

#[cfg(test)]
pub(crate) fn verified_asset_for_test(payload: &[u8], name: &str) -> VerifiedUpdateAsset {
    VerifiedUpdateAsset::for_test(payload, name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub body: String,
    pub is_newer: bool,
    pub expected_asset: String,
    pub asset: Option<VerifiedUpdateAsset>,
    pub trust_error: Option<UpdateTrustError>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpdateValidationError {
    #[error("release asset does not contain a trusted SHA-256 digest")]
    MissingDigest,
    #[error("automatic updates only accept HTTPS assets hosted by GitHub")]
    InvalidHost,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error(transparent)]
    Validation(#[from] UpdateValidationError),
    #[error("GitHub release request failed: {0}")]
    Request(String),
    #[error("GitHub latest release did not include a tag")]
    MissingReleaseTag,
    #[error("invalid release version: {0}")]
    InvalidVersion(String),
    #[error("update I/O failed: {0}")]
    Io(String),
    #[error("GitHub redirected the update download to an unexpected host")]
    InvalidRedirectHost,
    #[error("downloaded update is larger than the published release asset")]
    DownloadTooLarge,
    #[error("downloaded update size mismatch: expected {expected}, received {received}")]
    SizeMismatch { expected: u64, received: u64 },
    #[error("downloaded update failed SHA-256 verification")]
    DigestMismatch,
    #[error("update archive contains too many files")]
    TooManyArchiveFiles,
    #[error("update archive expands beyond the safety limit")]
    ExpandedArchiveTooLarge,
    #[error("unsafe path in update archive: {0}")]
    UnsafeArchivePath(String),
    #[error("symbolic links are not allowed in update archives: {0}")]
    ArchiveSymlink(String),
    #[error("special files are not allowed in update archives: {0}")]
    ArchiveSpecialFile(String),
    #[error("duplicate path in update archive: {0}")]
    DuplicateArchivePath(String),
    #[error("update archive failed: {0}")]
    Archive(String),
    #[error(transparent)]
    Trust(#[from] UpdateTrustError),
}

impl From<io::Error> for UpdateError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
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
    expected_asset_name_for(OS, ARCH)
}

pub fn expected_asset_name_for(os: &str, arch: &str) -> String {
    format!(
        "SSHMountMate-{}-{}.zip",
        platform_name(os),
        architecture_name(arch)
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
    let expected_prefix = format!("/{REPOSITORY}/releases/download/");
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("github.com" | "www.github.com"))
        || !url.path().starts_with(&expected_prefix)
    {
        return Err(UpdateValidationError::InvalidHost);
    }
    Ok(())
}

pub fn select_asset(assets: &[ReleaseAsset], expected_name: &str) -> Option<ReleaseAsset> {
    assets
        .iter()
        .find(|asset| asset.name == expected_name && !asset.url.is_empty())
        .cloned()
}

pub fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, UpdateError> {
    Ok(parse_version(left)?.cmp(&parse_version(right)?))
}

fn parse_version(value: &str) -> Result<Version, UpdateError> {
    let value = value.trim().trim_start_matches(['v', 'V']);
    if let Ok(version) = Version::parse(value) {
        return Ok(version);
    }

    let suffix_index = value.find(['-', '+']).unwrap_or(value.len());
    let (release, suffix) = value.split_at(suffix_index);
    let parts: Vec<_> = release.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(UpdateError::InvalidVersion(value.into()));
    }
    let normalized = format!(
        "{}.{}.{}{}",
        parts[0],
        parts.get(1).copied().unwrap_or("0"),
        parts.get(2).copied().unwrap_or("0"),
        suffix
    );
    Version::parse(&normalized).map_err(|_| UpdateError::InvalidVersion(value.into()))
}

pub fn check_for_updates(current_version: &str) -> Result<UpdateInfo, UpdateError> {
    let client = update_client(Duration::from_secs(15))?;
    if current_is_prerelease(current_version)? {
        let releases = fetch_releases_retry(&client).map_err(UpdateError::Request)?;
        let release = select_release_for_channel(current_version, releases)?;
        update_info_from_release(&client, current_version, release)
    } else {
        match fetch_latest_release(&client) {
            Ok(release) => update_info_from_release(&client, current_version, release),
            Err(api_error) => {
                fetch_latest_release_redirect(&client, current_version).map_err(|error| {
                    UpdateError::Request(format!(
                        "{api_error}; latest-release fallback failed: {error}"
                    ))
                })
            }
        }
    }
}

fn update_client(timeout: Duration) -> Result<Client, UpdateError> {
    Client::builder()
        .timeout(timeout)
        .redirect(Policy::custom(|attempt| {
            if trusted_update_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("update redirect was not an approved HTTPS GitHub URL")
            }
        }))
        .build()
        .map_err(|error| UpdateError::Request(error.to_string()))
}

fn trusted_update_url(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "www.github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

fn fetch_releases(client: &Client) -> Result<Vec<GithubRelease>, String> {
    let request = client
        .get(RELEASES_API)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "SSHMountMate-update-check");
    #[cfg(test)]
    let request = if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request.bearer_auth(token)
    } else {
        request
    };
    let response = request
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body = response.bytes().map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| format!("release list JSON is invalid: {error}"))
}

fn fetch_releases_retry(client: &Client) -> Result<Vec<GithubRelease>, String> {
    fetch_releases(client).or_else(|first_error| {
        fetch_releases(client)
            .map_err(|second_error| format!("{first_error}; retry failed: {second_error}"))
    })
}

fn fetch_latest_release(client: &Client) -> Result<GithubRelease, String> {
    let request = client
        .get(LATEST_RELEASE_API)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "SSHMountMate-update-check");
    #[cfg(test)]
    let request = if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request.bearer_auth(token)
    } else {
        request
    };
    let response = request
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body = response.bytes().map_err(|error| error.to_string())?;
    serde_json::from_slice(&body)
        .map_err(|error| format!("latest release JSON is invalid: {error}"))
}

fn current_is_prerelease(current_version: &str) -> Result<bool, UpdateError> {
    Ok(!parse_version(current_version)?.pre.is_empty())
}

fn select_release_for_channel(
    current_version: &str,
    releases: Vec<GithubRelease>,
) -> Result<GithubRelease, UpdateError> {
    let include_prereleases = current_is_prerelease(current_version)?;
    let mut candidates = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = parse_version(&release.tag_name).ok()?;
            let is_prerelease = release.prerelease || !version.pre.is_empty();
            (include_prereleases || !is_prerelease).then_some((version, release))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates
        .pop()
        .map(|(_, release)| release)
        .ok_or(UpdateError::MissingReleaseTag)
}

fn update_info_from_release(
    client: &Client,
    current_version: &str,
    release: GithubRelease,
) -> Result<UpdateInfo, UpdateError> {
    let latest_version = release.tag_name.trim().to_owned();
    if latest_version.is_empty() {
        return Err(UpdateError::MissingReleaseTag);
    }
    let expected_asset = expected_asset_name();
    let verification = verified_update_asset(client, &release, &expected_asset);
    let (asset, trust_error) = match verification {
        Ok(asset) => (Some(asset), None),
        Err(error) => (None, Some(error)),
    };
    let is_newer = compare_versions(current_version, &latest_version)?.is_lt();
    Ok(UpdateInfo {
        current_version: current_version.into(),
        release_name: if release.name.trim().is_empty() {
            latest_version.clone()
        } else {
            release.name
        },
        release_url: if release.html_url.trim().is_empty() {
            LATEST_RELEASE_PAGE.into()
        } else {
            release.html_url
        },
        latest_version,
        body: release.body,
        is_newer,
        expected_asset,
        asset,
        trust_error,
    })
}

fn verified_update_asset(
    client: &Client,
    release: &GithubRelease,
    expected_asset: &str,
) -> Result<VerifiedUpdateAsset, UpdateTrustError> {
    let manifest_asset = unique_release_asset(&release.assets, MANIFEST_ASSET_NAME)?;
    let signature_asset = unique_release_asset(&release.assets, SIGNATURE_ASSET_NAME)?;
    let manifest_bytes =
        fetch_small_release_asset(client, release, manifest_asset, MAX_MANIFEST_BYTES)?;
    let signature_bytes =
        fetch_small_release_asset(client, release, signature_asset, MAX_SIGNATURE_BYTES)?;

    let zip_assets = release
        .assets
        .iter()
        .filter(|asset| asset.name.starts_with("SSHMountMate-") && asset.name.ends_with(".zip"))
        .map(|asset| {
            let digest = verified_sha256(asset)
                .map_err(|_| UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()))?;
            Ok(PublishedAsset {
                name: asset.name.clone(),
                size: asset.size,
                digest: format!("sha256:{digest}"),
                url: asset.url.clone(),
            })
        })
        .collect::<Result<Vec<_>, UpdateTrustError>>()?;
    let trusted_keys = embedded_trusted_keys()?;
    let verified = verify_release_manifest(
        &manifest_bytes,
        &signature_bytes,
        &trusted_keys,
        &release.tag_name,
        release.prerelease,
        &zip_assets,
    )?;
    let signed = verified
        .asset(expected_asset)
        .ok_or_else(|| UpdateTrustError::ReleaseAssetMismatch(expected_asset.into()))?;
    let published = unique_release_asset(&release.assets, expected_asset)?;
    Ok(VerifiedUpdateAsset::from_verified(
        published,
        signed,
        verified.manifest().version.clone(),
        verified.manifest().channel,
        verified.manifest().key_id.clone(),
    ))
}

fn unique_release_asset<'a>(
    assets: &'a [ReleaseAsset],
    name: &str,
) -> Result<&'a ReleaseAsset, UpdateTrustError> {
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let asset = matches.next().ok_or_else(|| {
        if name == MANIFEST_ASSET_NAME {
            UpdateTrustError::MissingManifest
        } else if name == SIGNATURE_ASSET_NAME {
            UpdateTrustError::MissingSignature
        } else {
            UpdateTrustError::ReleaseAssetMismatch(name.into())
        }
    })?;
    if matches.next().is_some() {
        return Err(UpdateTrustError::ReleaseAssetMismatch(format!(
            "duplicate asset {name}"
        )));
    }
    Ok(asset)
}

fn fetch_small_release_asset(
    client: &Client,
    release: &GithubRelease,
    asset: &ReleaseAsset,
    maximum: usize,
) -> Result<Vec<u8>, UpdateTrustError> {
    if asset.size == 0 || asset.size > maximum as u64 {
        return Err(UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()));
    }
    if !release_asset_url_matches(asset, &release.tag_name) {
        return Err(UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()));
    }
    let expected_digest = verified_sha256(asset)
        .map_err(|_| UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()))?;
    let response = client
        .get(&asset.url)
        .header(USER_AGENT, "SSHMountMate-update-manifest")
        .send()
        .map_err(|error| UpdateTrustError::ReleaseAssetMismatch(error.to_string()))?
        .error_for_status()
        .map_err(|error| UpdateTrustError::ReleaseAssetMismatch(error.to_string()))?;
    if !trusted_update_url(response.url()) {
        return Err(UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()));
    }
    let body = response
        .bytes()
        .map_err(|error| UpdateTrustError::ReleaseAssetMismatch(error.to_string()))?;
    if body.len() as u64 != asset.size
        || body.len() > maximum
        || format!("{:x}", Sha256::digest(&body)) != expected_digest
    {
        return Err(UpdateTrustError::ReleaseAssetMismatch(asset.name.clone()));
    }
    Ok(body.to_vec())
}

fn release_asset_url_matches(asset: &ReleaseAsset, release_tag: &str) -> bool {
    let Ok(url) = Url::parse(&asset.url) else {
        return false;
    };
    url.scheme() == "https"
        && matches!(url.host_str(), Some("github.com" | "www.github.com"))
        && url.path()
            == format!(
                "/{REPOSITORY}/releases/download/{release_tag}/{}",
                asset.name
            )
}

fn fetch_latest_release_redirect(
    client: &Client,
    current_version: &str,
) -> Result<UpdateInfo, String> {
    let response = client
        .get(LATEST_RELEASE_PAGE)
        .header(USER_AGENT, "SSHMountMate-update-check")
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let final_url = response.url().clone();
    let marker = "/releases/tag/";
    let Some(index) = final_url.path().find(marker) else {
        return Err("latest-release redirect did not include a tag".into());
    };
    let encoded_tag = &final_url.path()[index + marker.len()..];
    let latest_version = percent_decode(encoded_tag)?;
    let is_newer = compare_versions(current_version, &latest_version)
        .map_err(|error| error.to_string())?
        .is_lt();
    Ok(UpdateInfo {
        current_version: current_version.into(),
        latest_version: latest_version.clone(),
        release_name: format!("SSH MountMate {latest_version}"),
        release_url: final_url.to_string(),
        body: String::new(),
        is_newer,
        expected_asset: expected_asset_name(),
        asset: None,
        trust_error: Some(UpdateTrustError::MissingManifest),
    })
}

fn percent_decode(value: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(pair) = bytes.get(index + 1..index + 3) else {
                return Err("release tag contains invalid percent encoding".into());
            };
            let text = std::str::from_utf8(pair)
                .map_err(|_| "release tag contains invalid percent encoding")?;
            decoded.push(
                u8::from_str_radix(text, 16)
                    .map_err(|_| "release tag contains invalid percent encoding")?,
            );
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "release tag is not valid UTF-8".into())
}

pub fn download_verified_asset(
    asset: &VerifiedUpdateAsset,
    destination: &Path,
    progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<PathBuf, UpdateError> {
    validate_verified_asset_url(asset)?;
    let client = update_client(Duration::from_secs(30))?;
    let response = client
        .get(&asset.url)
        .header(USER_AGENT, "SSHMountMate-self-update")
        .send()
        .map_err(|error| UpdateError::Request(error.to_string()))?
        .error_for_status()
        .map_err(|error| UpdateError::Request(error.to_string()))?;
    validate_redirect(response.url())?;
    write_verified_response(response, asset, destination, progress)
}

fn validate_redirect(url: &Url) -> Result<(), UpdateError> {
    if trusted_update_url(url) {
        Ok(())
    } else {
        Err(UpdateError::InvalidRedirectHost)
    }
}

fn write_verified_response(
    response: Response,
    asset: &VerifiedUpdateAsset,
    destination: &Path,
    progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<PathBuf, UpdateError> {
    let content_length = response.content_length().unwrap_or(0);
    write_verified_stream(response, asset, destination, content_length, progress)
}

fn write_verified_stream<R: Read>(
    mut reader: R,
    asset: &VerifiedUpdateAsset,
    destination: &Path,
    content_length: u64,
    mut progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<PathBuf, UpdateError> {
    let expected_digest = asset.sha256();
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!(
        "{}part",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .map_or(String::new(), |value| format!("{value}."))
    ));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }

    let total = asset.size;
    let limit = asset.size.min(DEFAULT_DOWNLOAD_LIMIT);
    if content_length > 0 && content_length > limit {
        return Err(UpdateError::DownloadTooLarge);
    }

    let result = (|| {
        let mut output = File::create(&temporary)?;
        let mut hasher = Sha256::new();
        let mut received = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            received = received.saturating_add(read as u64);
            if received > limit {
                return Err(UpdateError::DownloadTooLarge);
            }
            output.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            if let Some(callback) = progress.as_mut() {
                callback(received, total);
            }
        }
        output.flush()?;
        output.sync_all()?;

        if received != asset.size {
            return Err(UpdateError::SizeMismatch {
                expected: asset.size,
                received,
            });
        }
        let actual_digest = format!("{:x}", hasher.finalize());
        if actual_digest != expected_digest {
            return Err(UpdateError::DigestMismatch);
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    replace_download_transactionally(&temporary, destination)?;
    Ok(destination.to_owned())
}

fn validate_verified_asset_url(asset: &VerifiedUpdateAsset) -> Result<(), UpdateValidationError> {
    let Ok(url) = Url::parse(&asset.url) else {
        return Err(UpdateValidationError::InvalidHost);
    };
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("github.com" | "www.github.com"))
        || url.path()
            != format!(
                "/{REPOSITORY}/releases/download/v{}/{}",
                asset.version, asset.name
            )
    {
        return Err(UpdateValidationError::InvalidHost);
    }
    Ok(())
}

fn replace_download_transactionally(
    temporary: &Path,
    destination: &Path,
) -> Result<(), UpdateError> {
    if !destination.exists() {
        fs::rename(temporary, destination)?;
        return Ok(());
    }

    let backup = destination.with_extension(format!(
        "{}backup",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .map_or(String::new(), |value| format!("{value}."))
    ));
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    fs::rename(destination, &backup)?;
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(replace_error) => match fs::rename(&backup, destination) {
            Ok(()) => Err(UpdateError::Io(replace_error.to_string())),
            Err(restore_error) => Err(UpdateError::Io(format!(
                "could not install verified download ({replace_error}) or restore previous download ({restore_error}); backup retained at {}",
                backup.display()
            ))),
        },
    }
}

pub fn safe_extract_zip(archive: &Path, destination: &Path) -> Result<PathBuf, UpdateError> {
    if destination.exists() {
        let metadata = fs::symlink_metadata(destination)?;
        if metadata.file_type().is_symlink() {
            return Err(UpdateError::UnsafeArchivePath(
                destination.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            fs::remove_dir_all(destination)?;
        } else {
            fs::remove_file(destination)?;
        }
    }
    fs::create_dir_all(destination)?;
    let result = extract_zip_contents(archive, destination);
    if let Err(error) = result {
        let _ = fs::remove_dir_all(destination);
        return Err(error);
    }
    Ok(destination.to_owned())
}

fn extract_zip_contents(archive: &Path, destination: &Path) -> Result<(), UpdateError> {
    let file = File::open(archive)?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| UpdateError::Archive(error.to_string()))?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(UpdateError::TooManyArchiveFiles);
    }
    let root = destination.canonicalize()?;
    let mut expanded_size = 0_u64;
    let mut extracted_paths = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| UpdateError::Archive(error.to_string()))?;
        let raw_name = entry.name().to_owned();
        let Some(relative) = entry.enclosed_name() else {
            return Err(UpdateError::UnsafeArchivePath(raw_name));
        };
        if !safe_archive_relative(&raw_name, &relative) {
            return Err(UpdateError::UnsafeArchivePath(raw_name));
        }
        let normalized_path = relative.to_string_lossy().replace('\\', "/").to_lowercase();
        if !extracted_paths.insert(normalized_path) {
            return Err(UpdateError::DuplicateArchivePath(raw_name));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type == 0o120000 {
                return Err(UpdateError::ArchiveSymlink(raw_name));
            }
            let expected_type = if entry.is_dir() { 0o040000 } else { 0o100000 };
            if file_type != 0 && file_type != expected_type {
                return Err(UpdateError::ArchiveSpecialFile(raw_name));
            }
        }
        let expected_size = entry.size();
        expanded_size = expanded_size.saturating_add(expected_size);
        if expanded_size > MAX_EXTRACTED_SIZE {
            return Err(UpdateError::ExpandedArchiveTooLarge);
        }

        let target = root.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&target)?;
        let remaining_limit = MAX_EXTRACTED_SIZE - expanded_size + expected_size + 1;
        let copied = io::copy(&mut entry.by_ref().take(remaining_limit), &mut output)?;
        if copied != expected_size {
            return Err(UpdateError::Archive(format!(
                "entry size mismatch for {}",
                relative.display()
            )));
        }
        output.flush()?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

fn safe_archive_relative(raw_name: &str, relative: &Path) -> bool {
    if raw_name.contains('\\') {
        return false;
    }
    relative.components().all(|component| {
        let Component::Normal(value) = component else {
            return false;
        };
        let value = value.to_string_lossy();
        if value.is_empty()
            || value.contains(':')
            || value.contains(['<', '>', '"', '|', '?', '*'])
            || value.ends_with('.')
            || value.ends_with(' ')
            || value.chars().any(char::is_control)
        {
            return false;
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
        ) && !matches!(
            stem.as_str(),
            "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
        ) && !matches!(
            stem.as_str(),
            "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;

    fn asset_for(payload: &[u8]) -> ReleaseAsset {
        ReleaseAsset {
            name: "SSHMountMate-windows-x64.zip".into(),
            url: format!("https://github.com/{REPOSITORY}/releases/download/v1/file.zip"),
            digest: format!("sha256:{:x}", Sha256::digest(payload)),
            size: payload.len() as u64,
        }
    }

    fn verified_asset_for(payload: &[u8]) -> VerifiedUpdateAsset {
        VerifiedUpdateAsset::for_test(payload, "SSHMountMate-windows-x64.zip")
    }

    #[test]
    fn github_asset_uses_browser_download_url_not_api_url() {
        let asset: ReleaseAsset = serde_json::from_str(
            r#"{
                "name": "SSHMountMate-windows-x64.zip",
                "url": "https://api.github.com/repos/example/releases/assets/1",
                "browser_download_url": "https://github.com/Stardust0831/ssh-mountmate/releases/download/v1/file.zip",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "size": 1
            }"#,
        )
        .unwrap();
        assert_eq!(
            asset.url,
            "https://github.com/Stardust0831/ssh-mountmate/releases/download/v1/file.zip"
        );
    }

    fn release(tag: &str, prerelease: bool, draft: bool) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            name: String::new(),
            html_url: String::new(),
            body: String::new(),
            draft,
            prerelease,
            assets: Vec::new(),
        }
    }

    fn write_zip(path: &Path, write: impl FnOnce(&mut ZipWriter<File>)) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        write(&mut writer);
        writer.finish().unwrap();
    }

    #[test]
    fn asset_names_cover_release_matrix() {
        assert_eq!(platform_name("windows"), "windows");
        assert_eq!(platform_name("macos"), "macos");
        assert_eq!(architecture_name("aarch64"), "arm64");
        assert_eq!(architecture_name("x86_64"), "x64");
        assert_eq!(
            expected_asset_name_for("windows", "x86_64"),
            "SSHMountMate-windows-x64.zip"
        );
        assert_eq!(
            expected_asset_name_for("macos", "aarch64"),
            "SSHMountMate-macos-arm64.zip"
        );
        assert_eq!(
            [
                ("windows", "x86_64"),
                ("windows", "aarch64"),
                ("macos", "x86_64"),
                ("macos", "aarch64"),
                ("linux", "x86_64"),
                ("linux", "aarch64"),
            ]
            .map(|(os, arch)| expected_asset_name_for(os, arch)),
            [
                "SSHMountMate-windows-x64.zip",
                "SSHMountMate-windows-arm64.zip",
                "SSHMountMate-macos-x64.zip",
                "SSHMountMate-macos-arm64.zip",
                "SSHMountMate-linux-x64.zip",
                "SSHMountMate-linux-arm64.zip",
            ]
        );
    }

    #[test]
    fn semantic_versions_preserve_prerelease_ordering() {
        assert!(
            compare_versions("v0.4.0-alpha.1", "v0.4.0-rc.1")
                .unwrap()
                .is_lt()
        );
        assert!(compare_versions("v0.4.0-rc.10", "v0.4.0").unwrap().is_lt());
        assert!(compare_versions("1", "1.0.0").unwrap().is_eq());
        assert!(compare_versions("1.2", "1.2.1").unwrap().is_lt());
    }

    #[test]
    fn prerelease_channel_selects_the_highest_published_version() {
        let selected = select_release_for_channel(
            "v0.4.0-alpha.1",
            vec![
                release("v0.4.0-alpha.2", true, false),
                release("v0.4.0-alpha.3", true, false),
                release("v9.0.0-alpha.1", true, true),
            ],
        )
        .unwrap();
        assert_eq!(selected.tag_name, "v0.4.0-alpha.3");
    }

    #[test]
    fn stable_channel_ignores_prereleases_and_prerelease_tags() {
        let selected = select_release_for_channel(
            "v0.4.0",
            vec![
                release("v0.5.0-alpha.1", true, false),
                release("v0.5.0-beta.1", false, false),
                release("v0.4.1", false, false),
            ],
        )
        .unwrap();
        assert_eq!(selected.tag_name, "v0.4.1");
    }

    #[test]
    fn prerelease_channel_can_promote_to_a_newer_stable_release() {
        let selected = select_release_for_channel(
            "v0.4.0-rc.2",
            vec![
                release("v0.4.0-rc.3", true, false),
                release("v0.4.0", false, false),
            ],
        )
        .unwrap();
        assert_eq!(selected.tag_name, "v0.4.0");
    }

    #[test]
    #[ignore = "requires the live GitHub releases API"]
    fn live_release_channels_decode_and_select_expected_versions() {
        let client = update_client(Duration::from_secs(30)).unwrap();
        let releases = fetch_releases_retry(&client).unwrap();
        let preview = select_release_for_channel("v0.4.0-alpha.1", releases).unwrap();
        assert!(
            compare_versions("v0.4.0-alpha.1", &preview.tag_name)
                .unwrap()
                .is_lt()
        );

        let stable = fetch_latest_release(&client).unwrap();
        assert!(parse_version(&stable.tag_name).unwrap().pre.is_empty());
        assert!(!stable.prerelease);
        assert!(!stable.draft);
    }

    #[test]
    fn release_asset_selection_is_exact() {
        let expected = ReleaseAsset {
            name: "SSHMountMate-linux-arm64.zip".into(),
            url: "https://github.com/example/arm64.zip".into(),
            digest: String::new(),
            size: 0,
        };
        let misleading = ReleaseAsset {
            name: "old-SSHMountMate-linux-arm64.zip.backup".into(),
            ..expected.clone()
        };
        assert_eq!(
            select_asset(&[misleading, expected.clone()], &expected.name),
            Some(expected)
        );
    }

    #[test]
    fn digest_and_url_must_be_trusted() {
        let asset = asset_for(b"verified update");
        assert_eq!(
            verified_sha256(&asset).unwrap(),
            format!("{:x}", Sha256::digest(b"verified update"))
        );
        assert_eq!(validate_asset_url(&asset), Ok(()));

        let mut other_repository = asset.clone();
        other_repository.url =
            "https://github.com/example/project/releases/download/v1/file.zip".into();
        assert_eq!(
            validate_asset_url(&other_repository),
            Err(UpdateValidationError::InvalidHost)
        );
    }

    #[test]
    fn every_update_redirect_must_remain_on_approved_https_hosts() {
        assert!(trusted_update_url(
            &Url::parse("https://release-assets.githubusercontent.com/file").unwrap()
        ));
        assert!(!trusted_update_url(
            &Url::parse("http://github.com/example").unwrap()
        ));
        assert!(!trusted_update_url(
            &Url::parse("https://github.example.com/file").unwrap()
        ));
    }

    #[test]
    fn release_info_never_exposes_an_unverified_asset_for_installation() {
        let expected_name = expected_asset_name();
        let release = GithubRelease {
            tag_name: "v99.0.0".into(),
            name: String::new(),
            html_url: String::new(),
            body: String::new(),
            draft: false,
            prerelease: false,
            assets: vec![ReleaseAsset {
                name: expected_name,
                url: format!(
                    "https://github.com/{REPOSITORY}/releases/download/v99.0.0/update.zip"
                ),
                digest: String::new(),
                size: 1,
            }],
        };

        assert!(
            update_info_from_release(
                &update_client(Duration::from_secs(1)).unwrap(),
                "1.0.0",
                release,
            )
            .unwrap()
            .asset
            .is_none()
        );
    }

    #[test]
    fn verified_stream_checks_size_digest_and_cleans_partial_file() {
        let temp = tempdir().unwrap();
        let destination = temp.path().join("app.zip");
        let payload = b"verified update";
        let asset = verified_asset_for(payload);
        let mut progress = Vec::new();
        write_verified_stream(
            Cursor::new(payload),
            &asset,
            &destination,
            payload.len() as u64,
            Some(&mut |current, total| progress.push((current, total))),
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), payload);
        assert_eq!(
            progress.last(),
            Some(&(payload.len() as u64, payload.len() as u64))
        );

        let mut invalid = asset.clone();
        invalid.sha256 = "0".repeat(64);
        assert!(matches!(
            write_verified_stream(Cursor::new(payload), &invalid, &destination, 0, None),
            Err(UpdateError::DigestMismatch)
        ));
        assert!(!destination.with_extension("zip.part").exists());
        assert_eq!(fs::read(&destination).unwrap(), payload);
    }

    #[test]
    fn verified_stream_replaces_an_existing_valid_download() {
        let temp = tempdir().unwrap();
        let destination = temp.path().join("app.zip");
        fs::write(&destination, b"old verified update").unwrap();
        let payload = b"new verified update";

        write_verified_stream(
            Cursor::new(payload),
            &verified_asset_for(payload),
            &destination,
            payload.len() as u64,
            None,
        )
        .unwrap();

        assert_eq!(fs::read(&destination).unwrap(), payload);
        assert!(!destination.with_extension("zip.backup").exists());
    }

    #[test]
    fn failed_download_replacement_restores_the_previous_file() {
        let temp = tempdir().unwrap();
        let destination = temp.path().join("app.zip");
        fs::write(&destination, b"old verified update").unwrap();

        assert!(
            replace_download_transactionally(&temp.path().join("missing.zip.part"), &destination)
                .is_err()
        );

        assert_eq!(fs::read(&destination).unwrap(), b"old verified update");
        assert!(!destination.with_extension("zip.backup").exists());
    }

    #[test]
    fn safe_extract_rejects_parent_traversal_and_cleans_destination() {
        let temp = tempdir().unwrap();
        let archive = temp.path().join("bad.zip");
        write_zip(&archive, |writer| {
            writer
                .start_file("../outside.txt", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"bad").unwrap();
        });
        let destination = temp.path().join("out");
        assert!(matches!(
            safe_extract_zip(&archive, &destination),
            Err(UpdateError::UnsafeArchivePath(_))
        ));
        assert!(!destination.exists());
        assert!(!temp.path().join("outside.txt").exists());
    }

    #[test]
    fn safe_extract_rejects_symbolic_links() {
        let temp = tempdir().unwrap();
        let archive = temp.path().join("link.zip");
        write_zip(&archive, |writer| {
            writer
                .add_symlink(
                    "SSHMountMate/link",
                    "../outside",
                    SimpleFileOptions::default(),
                )
                .unwrap();
        });
        assert!(matches!(
            safe_extract_zip(&archive, &temp.path().join("out")),
            Err(UpdateError::ArchiveSymlink(_))
        ));
    }

    #[test]
    fn safe_extract_rejects_cross_platform_unsafe_names() {
        for (index, name) in [
            "SSHMountMate/file:stream",
            "SSHMountMate/NUL.txt",
            "SSHMountMate/report?.txt",
            "SSHMountMate/CONOUT$.txt",
            "folder\\file",
        ]
        .into_iter()
        .enumerate()
        {
            let temp = tempdir().unwrap();
            let archive = temp.path().join(format!("unsafe-{index}.zip"));
            write_zip(&archive, |writer| {
                writer
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(b"bad").unwrap();
            });
            assert!(matches!(
                safe_extract_zip(&archive, &temp.path().join("out")),
                Err(UpdateError::UnsafeArchivePath(_))
            ));
        }
    }

    #[test]
    fn safe_extract_rejects_case_colliding_paths() {
        let temp = tempdir().unwrap();
        let archive = temp.path().join("duplicate.zip");
        write_zip(&archive, |writer| {
            for name in ["SSHMountMate/File.txt", "SSHMountMate/file.txt"] {
                writer
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(name.as_bytes()).unwrap();
            }
        });
        assert!(matches!(
            safe_extract_zip(&archive, &temp.path().join("out")),
            Err(UpdateError::DuplicateArchivePath(_))
        ));
    }

    #[test]
    fn safe_extract_writes_regular_files() {
        let temp = tempdir().unwrap();
        let archive = temp.path().join("good.zip");
        write_zip(&archive, |writer| {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o755);
            writer
                .start_file("SSHMountMate/SSHMountMate", options)
                .unwrap();
            writer.write_all(b"binary").unwrap();
        });
        let destination = temp.path().join("out");
        safe_extract_zip(&archive, &destination).unwrap();
        assert_eq!(
            fs::read(destination.join("SSHMountMate/SSHMountMate")).unwrap(),
            b"binary"
        );
    }
}
