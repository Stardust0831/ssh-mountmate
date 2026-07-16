use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use mountmate_core::update_manifest::{
    ManifestAsset, PublishedAsset, ReleaseChannel, SignedUpdateManifestV1, TrustedUpdateKey,
    TrustedUpdateKeySet, UpdateTrustError, build_release_manifest, embedded_trusted_keys,
    public_key_record, sign_manifest, verify_local_release_set, verify_release_manifest,
};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

const ASSET_NAMES: [&str; 6] = [
    "SSHMountMate-linux-arm64.zip",
    "SSHMountMate-linux-x64.zip",
    "SSHMountMate-macos-arm64.zip",
    "SSHMountMate-macos-x64.zip",
    "SSHMountMate-windows-arm64.zip",
    "SSHMountMate-windows-x64.zip",
];

fn test_key() -> (String, mountmate_core::update_manifest::TrustedUpdateKey) {
    let signing = SigningKey::generate(&mut OsRng);
    let private = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        signing.to_pkcs8_der().unwrap().as_bytes(),
    );
    let public = public_key_record(&signing.verifying_key());
    (private, public)
}

fn manifest_for(key_id: &str) -> SignedUpdateManifestV1 {
    SignedUpdateManifestV1 {
        schema_version: 1,
        key_id: key_id.into(),
        version: "0.4.1-alpha.1".into(),
        channel: ReleaseChannel::Prerelease,
        assets: ASSET_NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let payload = format!("payload-{index}");
                ManifestAsset {
                    name: (*name).into(),
                    size: payload.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(payload.as_bytes())),
                }
            })
            .collect(),
    }
}

fn published(manifest: &SignedUpdateManifestV1) -> Vec<PublishedAsset> {
    manifest
        .assets
        .iter()
        .map(|asset| PublishedAsset {
            name: asset.name.clone(),
            size: asset.size,
            digest: format!("sha256:{}", asset.sha256),
            url: format!(
                "https://github.com/Stardust0831/ssh-mountmate/releases/download/v{}/{}",
                manifest.version, asset.name
            ),
        })
        .collect()
}

#[test]
fn correctly_signed_release_produces_verified_platform_asset() {
    let (private, public) = test_key();
    let manifest = manifest_for(&public.key_id);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = sign_manifest(&private, &bytes).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();

    let verified = verify_release_manifest(
        &bytes,
        signature.as_bytes(),
        &keys,
        "v0.4.1-alpha.1",
        true,
        &published(&manifest),
    )
    .unwrap();

    assert_eq!(
        verified.asset("SSHMountMate-linux-x64.zip").unwrap().name,
        "SSHMountMate-linux-x64.zip"
    );
}

#[test]
fn manifest_tampering_and_unknown_keys_are_rejected() {
    let (private, public) = test_key();
    let manifest = manifest_for(&public.key_id);
    let mut bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = sign_manifest(&private, &bytes).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();
    *bytes.last_mut().unwrap() ^= 1;

    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &published(&manifest),
        ),
        Err(UpdateTrustError::InvalidManifest(_) | UpdateTrustError::InvalidSignature)
    ));

    let (other_private, other_public) = test_key();
    let other_manifest = manifest_for(&other_public.key_id);
    let other_bytes = serde_json::to_vec(&other_manifest).unwrap();
    let other_signature = sign_manifest(&other_private, &other_bytes).unwrap();
    assert!(matches!(
        verify_release_manifest(
            &other_bytes,
            other_signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &published(&other_manifest),
        ),
        Err(UpdateTrustError::UnknownKeyId(_))
    ));
}

#[test]
fn release_metadata_and_channel_must_match_signed_manifest() {
    let (private, public) = test_key();
    let manifest = manifest_for(&public.key_id);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = sign_manifest(&private, &bytes).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();
    let mut assets = published(&manifest);
    assets[0].size += 1;

    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &assets,
        ),
        Err(UpdateTrustError::ReleaseAssetMismatch(_))
    ));
    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            false,
            &published(&manifest),
        ),
        Err(UpdateTrustError::ChannelMismatch)
    ));
}

#[test]
fn identity_verification_rejects_a_signed_manifest_without_every_platform() {
    let (private, public) = test_key();
    let mut manifest = manifest_for(&public.key_id);
    manifest.assets.pop();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    assert!(matches!(
        sign_manifest(&private, &bytes),
        Err(UpdateTrustError::InvalidAssetSet(_))
    ));
}

#[test]
fn multiple_public_keys_support_rotation() {
    let (old_private, old_public) = test_key();
    let (new_private, new_public) = test_key();
    let keys = TrustedUpdateKeySet::new(vec![old_public.clone(), new_public.clone()]).unwrap();

    for (private, public) in [(old_private, old_public), (new_private, new_public)] {
        let manifest = manifest_for(&public.key_id);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let signature = sign_manifest(&private, &bytes).unwrap();
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &published(&manifest),
        )
        .unwrap();
    }
}

#[test]
fn missing_or_wrong_signature_cannot_be_replaced_by_github_digest() {
    let (private, public) = test_key();
    let manifest = manifest_for(&public.key_id);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();
    let assets = published(&manifest);

    assert!(matches!(
        verify_release_manifest(&bytes, b"", &keys, "v0.4.1-alpha.1", true, &assets,),
        Err(UpdateTrustError::MissingSignature)
    ));
    let (_, wrong_public) = test_key();
    let wrong_signature = sign_manifest(&private, &bytes).unwrap();
    let wrong_key = TrustedUpdateKey {
        key_id: wrong_public.key_id,
        public_key: wrong_public.public_key,
    };
    let wrong_keys = TrustedUpdateKeySet::new(vec![wrong_key]).unwrap();
    assert!(matches!(
        verify_release_manifest(
            &bytes,
            wrong_signature.as_bytes(),
            &wrong_keys,
            "v0.4.1-alpha.1",
            true,
            &assets,
        ),
        Err(UpdateTrustError::UnknownKeyId(_))
    ));
}

#[test]
fn version_digest_and_asset_replacement_are_rejected() {
    let (private, public) = test_key();
    let manifest = manifest_for(&public.key_id);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = sign_manifest(&private, &bytes).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();

    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.2-alpha.1",
            true,
            &published(&manifest),
        ),
        Err(UpdateTrustError::VersionMismatch)
    ));

    let mut replaced = published(&manifest);
    replaced[2].digest = format!("sha256:{}", "0".repeat(64));
    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &replaced,
        ),
        Err(UpdateTrustError::ReleaseAssetMismatch(_))
    ));
    replaced[2].digest.clear();
    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &replaced,
        ),
        Err(UpdateTrustError::ReleaseAssetMismatch(_))
    ));

    let mut wrong_url = published(&manifest);
    wrong_url[4].url = wrong_url[4]
        .url
        .replace("/v0.4.1-alpha.1/", "/v0.4.1-alpha.2/");
    assert!(matches!(
        verify_release_manifest(
            &bytes,
            signature.as_bytes(),
            &keys,
            "v0.4.1-alpha.1",
            true,
            &wrong_url,
        ),
        Err(UpdateTrustError::ReleaseAssetMismatch(_))
    ));
}

#[test]
fn local_zip_tampering_is_detected_after_manifest_signing() {
    let temp = tempfile::tempdir().unwrap();
    for (index, name) in ASSET_NAMES.iter().enumerate() {
        std::fs::write(temp.path().join(name), format!("zip-{index}")).unwrap();
    }
    let (private, public) = test_key();
    let manifest = build_release_manifest(
        temp.path(),
        "0.4.1-alpha.1",
        ReleaseChannel::Prerelease,
        &public.key_id,
    )
    .unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = sign_manifest(&private, &bytes).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();
    std::fs::write(temp.path().join(ASSET_NAMES[0]), b"replaced zip").unwrap();

    assert!(matches!(
        verify_local_release_set(&bytes, signature.as_bytes(), &keys, temp.path()),
        Err(UpdateTrustError::ReleaseAssetMismatch(_))
    ));
}

#[test]
fn duplicate_key_ids_are_rejected_before_verification() {
    let (_, key) = test_key();
    assert!(matches!(
        TrustedUpdateKeySet::new(vec![key.clone(), key]),
        Err(UpdateTrustError::InvalidKeyRegistry(_))
    ));
}

#[test]
fn key_registry_is_strict_and_binds_key_id_to_the_full_public_key() {
    let (_, key) = test_key();
    let valid = serde_json::json!({
        "schema_version": 1,
        "keys": [key.clone()],
    });
    assert!(
        !TrustedUpdateKeySet::from_json(&serde_json::to_vec(&valid).unwrap())
            .unwrap()
            .is_empty()
    );

    let mut mismatched = key;
    mismatched.key_id = "ed25519-0000000000000000".into();
    let invalid = serde_json::json!({
        "schema_version": 1,
        "keys": [mismatched],
    });
    assert!(matches!(
        TrustedUpdateKeySet::from_json(&serde_json::to_vec(&invalid).unwrap()),
        Err(UpdateTrustError::InvalidKeyRegistry(_))
    ));

    let unknown_field = serde_json::json!({
        "schema_version": 1,
        "keys": [],
        "fallback_key": "not allowed",
    });
    assert!(matches!(
        TrustedUpdateKeySet::from_json(&serde_json::to_vec(&unknown_field).unwrap()),
        Err(UpdateTrustError::InvalidKeyRegistry(_))
    ));
}

#[test]
fn production_build_embeds_at_least_one_valid_trusted_update_key() {
    assert!(!embedded_trusted_keys().unwrap().is_empty());
}

#[test]
fn local_verification_rejects_noncanonical_asset_paths_before_reading_them() {
    let temp = tempfile::tempdir().unwrap();
    let (_, public) = test_key();
    let mut manifest = manifest_for(&public.key_id);
    manifest.assets[0].name = "../../outside.zip".into();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let keys = TrustedUpdateKeySet::new(vec![public]).unwrap();

    assert!(matches!(
        verify_local_release_set(&bytes, b"not a signature", &keys, temp.path()),
        Err(UpdateTrustError::InvalidAssetSet(_))
    ));
}
