use std::collections::HashSet;
use std::fs;
use std::path::Path;

use base64::Engine;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

pub const MANIFEST_ASSET_NAME: &str = "SSHMountMate-update-manifest-v1.json";
pub const SIGNATURE_ASSET_NAME: &str = "SSHMountMate-update-manifest-v1.sig";
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_SIGNATURE_BYTES: usize = 1024;
pub const MAX_KEY_REGISTRY_BYTES: usize = 64 * 1024;

pub const CANONICAL_ASSET_NAMES: [&str; 6] = [
    "SSHMountMate-linux-arm64.zip",
    "SSHMountMate-linux-x64.zip",
    "SSHMountMate-macos-arm64.zip",
    "SSHMountMate-macos-x64.zip",
    "SSHMountMate-windows-arm64.zip",
    "SSHMountMate-windows-x64.zip",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    Stable,
    Prerelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAsset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedUpdateManifestV1 {
    pub schema_version: u32,
    pub key_id: String,
    pub version: String,
    pub channel: ReleaseChannel,
    pub assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedUpdateKey {
    pub key_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedUpdateKeyFile {
    schema_version: u32,
    keys: Vec<TrustedUpdateKey>,
}

#[derive(Debug, Clone)]
pub struct TrustedUpdateKeySet {
    keys: Vec<(String, VerifyingKey)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAsset {
    pub name: String,
    pub size: u64,
    pub digest: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUpdateManifest {
    manifest: SignedUpdateManifestV1,
}

impl VerifiedUpdateManifest {
    pub fn manifest(&self) -> &SignedUpdateManifestV1 {
        &self.manifest
    }

    pub fn asset(&self, name: &str) -> Option<&ManifestAsset> {
        self.manifest.assets.iter().find(|asset| asset.name == name)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UpdateTrustError {
    #[error("the update manifest is missing")]
    MissingManifest,
    #[error("the update signature is missing")]
    MissingSignature,
    #[error("the update manifest is too large")]
    ManifestTooLarge,
    #[error("the update signature is too large")]
    SignatureTooLarge,
    #[error("invalid update manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid trusted update key registry: {0}")]
    InvalidKeyRegistry(String),
    #[error("unknown update signing key: {0}")]
    UnknownKeyId(String),
    #[error("the update manifest signature is invalid")]
    InvalidSignature,
    #[error("the update manifest version does not match the GitHub Release")]
    VersionMismatch,
    #[error("the update manifest channel does not match the GitHub Release")]
    ChannelMismatch,
    #[error("the signed update asset set is invalid: {0}")]
    InvalidAssetSet(String),
    #[error("the signed update manifest does not match the GitHub Release asset: {0}")]
    ReleaseAssetMismatch(String),
    #[error("invalid Ed25519 private key")]
    InvalidPrivateKey,
    #[error("the private key does not match manifest key_id {0}")]
    PrivateKeyMismatch(String),
    #[error("update signing I/O failed: {0}")]
    Io(String),
}

pub fn generate_signing_key() -> Result<(Zeroizing<String>, TrustedUpdateKey), UpdateTrustError> {
    let signing = SigningKey::generate(&mut OsRng);
    let document = signing
        .to_pkcs8_der()
        .map_err(|_| UpdateTrustError::InvalidPrivateKey)?;
    let private =
        Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(document.as_bytes()));
    let public = public_key_record(&signing.verifying_key());
    Ok((private, public))
}

pub fn build_release_manifest(
    assets_directory: &Path,
    version: &str,
    channel: ReleaseChannel,
    key_id: &str,
) -> Result<SignedUpdateManifestV1, UpdateTrustError> {
    let mut assets = Vec::with_capacity(CANONICAL_ASSET_NAMES.len());
    for name in CANONICAL_ASSET_NAMES {
        let path = assets_directory.join(name);
        let bytes = fs::read(&path).map_err(|error| {
            UpdateTrustError::Io(format!("could not read {}: {error}", path.display()))
        })?;
        if bytes.is_empty() {
            return Err(UpdateTrustError::InvalidAssetSet(format!(
                "{name} has zero size"
            )));
        }
        assets.push(ManifestAsset {
            name: name.into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        });
    }
    let manifest = SignedUpdateManifestV1 {
        schema_version: MANIFEST_SCHEMA_VERSION,
        key_id: key_id.into(),
        version: version.into(),
        channel,
        assets,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn verify_local_release_set(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &TrustedUpdateKeySet,
    assets_directory: &Path,
) -> Result<VerifiedUpdateManifest, UpdateTrustError> {
    let manifest: SignedUpdateManifestV1 = serde_json::from_slice(manifest_bytes)
        .map_err(|error| UpdateTrustError::InvalidManifest(error.to_string()))?;
    validate_manifest(&manifest)?;
    let release_tag = format!("v{}", manifest.version);
    let prerelease = manifest.channel == ReleaseChannel::Prerelease;
    let mut published = Vec::with_capacity(manifest.assets.len());
    for asset in &manifest.assets {
        let path = assets_directory.join(&asset.name);
        let bytes = fs::read(&path).map_err(|error| {
            UpdateTrustError::Io(format!("could not read {}: {error}", path.display()))
        })?;
        published.push(PublishedAsset {
            name: asset.name.clone(),
            size: bytes.len() as u64,
            digest: format!("sha256:{:x}", Sha256::digest(&bytes)),
            url: format!(
                "https://github.com/Stardust0831/ssh-mountmate/releases/download/{release_tag}/{}",
                asset.name
            ),
        });
    }
    verify_release_manifest(
        manifest_bytes,
        signature_bytes,
        trusted_keys,
        &release_tag,
        prerelease,
        &published,
    )
}

impl TrustedUpdateKeySet {
    pub fn new(keys: Vec<TrustedUpdateKey>) -> Result<Self, UpdateTrustError> {
        let mut ids = HashSet::new();
        let mut parsed = Vec::with_capacity(keys.len());
        for key in keys {
            if !valid_key_id(&key.key_id) {
                return Err(UpdateTrustError::InvalidKeyRegistry(format!(
                    "invalid key_id {}",
                    key.key_id
                )));
            }
            if !ids.insert(key.key_id.clone()) {
                return Err(UpdateTrustError::InvalidKeyRegistry(format!(
                    "duplicate key_id {}",
                    key.key_id
                )));
            }
            let decoded = decode_base64(&key.public_key).map_err(|_| {
                UpdateTrustError::InvalidKeyRegistry(format!(
                    "public key {} is not valid base64",
                    key.key_id
                ))
            })?;
            let bytes: [u8; 32] = decoded.try_into().map_err(|_| {
                UpdateTrustError::InvalidKeyRegistry(format!(
                    "public key {} is not 32 bytes",
                    key.key_id
                ))
            })?;
            let verifying = VerifyingKey::from_bytes(&bytes).map_err(|_| {
                UpdateTrustError::InvalidKeyRegistry(format!(
                    "public key {} is not Ed25519",
                    key.key_id
                ))
            })?;
            let expected = key_id_for(&verifying);
            if key.key_id != expected {
                return Err(UpdateTrustError::InvalidKeyRegistry(format!(
                    "key_id {} does not match public key fingerprint {expected}",
                    key.key_id
                )));
            }
            parsed.push((key.key_id, verifying));
        }
        Ok(Self { keys: parsed })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, UpdateTrustError> {
        if bytes.len() > MAX_KEY_REGISTRY_BYTES {
            return Err(UpdateTrustError::InvalidKeyRegistry(
                "registry is too large".into(),
            ));
        }
        let file: TrustedUpdateKeyFile = serde_json::from_slice(bytes)
            .map_err(|error| UpdateTrustError::InvalidKeyRegistry(error.to_string()))?;
        if file.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(UpdateTrustError::InvalidKeyRegistry(format!(
                "unsupported schema_version {}",
                file.schema_version
            )));
        }
        Self::new(file.keys)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.keys
            .iter()
            .find_map(|(candidate, key)| (candidate == key_id).then_some(key))
    }
}

pub fn embedded_trusted_keys() -> Result<TrustedUpdateKeySet, UpdateTrustError> {
    TrustedUpdateKeySet::from_json(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../distribution/update-public-keys.json"
    )))
}

pub fn public_key_record(verifying_key: &VerifyingKey) -> TrustedUpdateKey {
    TrustedUpdateKey {
        key_id: key_id_for(verifying_key),
        public_key: base64::engine::general_purpose::STANDARD.encode(verifying_key.as_bytes()),
    }
}

pub fn sign_manifest(
    private_key_pkcs8_base64: &str,
    manifest_bytes: &[u8],
) -> Result<String, UpdateTrustError> {
    if manifest_bytes.is_empty() {
        return Err(UpdateTrustError::MissingManifest);
    }
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateTrustError::ManifestTooLarge);
    }
    let manifest: SignedUpdateManifestV1 = serde_json::from_slice(manifest_bytes)
        .map_err(|error| UpdateTrustError::InvalidManifest(error.to_string()))?;
    validate_manifest(&manifest)?;
    let decoded = Zeroizing::new(
        decode_base64(private_key_pkcs8_base64.trim())
            .map_err(|_| UpdateTrustError::InvalidPrivateKey)?,
    );
    let signing =
        SigningKey::from_pkcs8_der(&decoded).map_err(|_| UpdateTrustError::InvalidPrivateKey)?;
    let actual_key_id = key_id_for(&signing.verifying_key());
    if manifest.key_id != actual_key_id {
        return Err(UpdateTrustError::PrivateKeyMismatch(manifest.key_id));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(signing.sign(manifest_bytes).to_bytes()))
}

pub fn verify_release_manifest(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &TrustedUpdateKeySet,
    release_tag: &str,
    release_prerelease: bool,
    published_assets: &[PublishedAsset],
) -> Result<VerifiedUpdateManifest, UpdateTrustError> {
    let verified = verify_release_manifest_identity(
        manifest_bytes,
        signature_bytes,
        trusted_keys,
        release_tag,
        release_prerelease,
    )?;
    validate_published_assets(&verified.manifest, release_tag, published_assets)?;
    Ok(verified)
}

pub(crate) fn verify_release_manifest_identity(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &TrustedUpdateKeySet,
    release_tag: &str,
    release_prerelease: bool,
) -> Result<VerifiedUpdateManifest, UpdateTrustError> {
    if manifest_bytes.is_empty() {
        return Err(UpdateTrustError::MissingManifest);
    }
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateTrustError::ManifestTooLarge);
    }
    if signature_bytes.is_empty() {
        return Err(UpdateTrustError::MissingSignature);
    }
    if signature_bytes.len() > MAX_SIGNATURE_BYTES {
        return Err(UpdateTrustError::SignatureTooLarge);
    }
    let manifest: SignedUpdateManifestV1 = serde_json::from_slice(manifest_bytes)
        .map_err(|error| UpdateTrustError::InvalidManifest(error.to_string()))?;
    let key = trusted_keys
        .get(&manifest.key_id)
        .ok_or_else(|| UpdateTrustError::UnknownKeyId(manifest.key_id.clone()))?;
    let signature = decode_base64(
        std::str::from_utf8(signature_bytes)
            .map_err(|_| UpdateTrustError::InvalidSignature)?
            .trim(),
    )
    .map_err(|_| UpdateTrustError::InvalidSignature)?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| UpdateTrustError::InvalidSignature)?;
    key.verify_strict(manifest_bytes, &Signature::from_bytes(&signature))
        .map_err(|_| UpdateTrustError::InvalidSignature)?;

    validate_manifest(&manifest)?;
    validate_release_identity(&manifest, release_tag, release_prerelease)?;
    Ok(VerifiedUpdateManifest { manifest })
}

fn validate_manifest(manifest: &SignedUpdateManifestV1) -> Result<(), UpdateTrustError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(UpdateTrustError::InvalidManifest(format!(
            "unsupported schema_version {}",
            manifest.schema_version
        )));
    }
    if !valid_key_id(&manifest.key_id) {
        return Err(UpdateTrustError::InvalidManifest(
            "key_id is not a canonical Ed25519 fingerprint".into(),
        ));
    }
    let version = Version::parse(&manifest.version)
        .map_err(|_| UpdateTrustError::InvalidManifest("version is not valid SemVer".into()))?;
    if version.to_string() != manifest.version {
        return Err(UpdateTrustError::InvalidManifest(
            "version is not canonical SemVer".into(),
        ));
    }
    let expected_channel = if version.pre.is_empty() {
        ReleaseChannel::Stable
    } else {
        ReleaseChannel::Prerelease
    };
    if manifest.channel != expected_channel {
        return Err(UpdateTrustError::ChannelMismatch);
    }
    if manifest.assets.len() != CANONICAL_ASSET_NAMES.len() {
        return Err(UpdateTrustError::InvalidAssetSet(format!(
            "expected {} assets, got {}",
            CANONICAL_ASSET_NAMES.len(),
            manifest.assets.len()
        )));
    }
    for (asset, expected_name) in manifest.assets.iter().zip(CANONICAL_ASSET_NAMES) {
        if asset.name != expected_name {
            return Err(UpdateTrustError::InvalidAssetSet(format!(
                "expected sorted asset {expected_name}, got {}",
                asset.name
            )));
        }
        if asset.size == 0 {
            return Err(UpdateTrustError::InvalidAssetSet(format!(
                "{} has zero size",
                asset.name
            )));
        }
        if !is_lower_sha256(&asset.sha256) {
            return Err(UpdateTrustError::InvalidAssetSet(format!(
                "{} has invalid SHA-256",
                asset.name
            )));
        }
    }
    Ok(())
}

fn validate_release_identity(
    manifest: &SignedUpdateManifestV1,
    release_tag: &str,
    release_prerelease: bool,
) -> Result<(), UpdateTrustError> {
    let Some(tag_version) = release_tag.trim().strip_prefix('v') else {
        return Err(UpdateTrustError::VersionMismatch);
    };
    if tag_version != manifest.version {
        return Err(UpdateTrustError::VersionMismatch);
    }
    let expected_prerelease = manifest.channel == ReleaseChannel::Prerelease;
    if release_prerelease != expected_prerelease {
        return Err(UpdateTrustError::ChannelMismatch);
    }
    Ok(())
}

fn validate_published_assets(
    manifest: &SignedUpdateManifestV1,
    release_tag: &str,
    published_assets: &[PublishedAsset],
) -> Result<(), UpdateTrustError> {
    if published_assets.len() != manifest.assets.len() {
        return Err(UpdateTrustError::ReleaseAssetMismatch(format!(
            "expected {} ZIP assets, got {}",
            manifest.assets.len(),
            published_assets.len()
        )));
    }
    let mut names = HashSet::new();
    for published in published_assets {
        if !names.insert(published.name.as_str()) {
            return Err(UpdateTrustError::ReleaseAssetMismatch(format!(
                "duplicate asset {}",
                published.name
            )));
        }
        let signed = manifest
            .assets
            .iter()
            .find(|asset| asset.name == published.name)
            .ok_or_else(|| UpdateTrustError::ReleaseAssetMismatch(published.name.clone()))?;
        if published.size != signed.size
            || published.digest != format!("sha256:{}", signed.sha256)
            || !asset_url_matches(&published.url, release_tag, &published.name)
        {
            return Err(UpdateTrustError::ReleaseAssetMismatch(
                published.name.clone(),
            ));
        }
    }
    Ok(())
}

fn asset_url_matches(value: &str, release_tag: &str, asset_name: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && matches!(url.host_str(), Some("github.com" | "www.github.com"))
        && url.path()
            == format!("/Stardust0831/ssh-mountmate/releases/download/{release_tag}/{asset_name}")
}

fn valid_key_id(value: &str) -> bool {
    value.len() == "ed25519-".len() + 16
        && value.starts_with("ed25519-")
        && value["ed25519-".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn key_id_for(verifying_key: &VerifyingKey) -> String {
    let digest = format!("{:x}", Sha256::digest(verifying_key.as_bytes()));
    format!("ed25519-{}", &digest[..16])
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn decode_base64(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(value)
}
