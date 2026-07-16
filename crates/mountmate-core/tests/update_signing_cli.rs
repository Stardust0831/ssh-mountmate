use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::json;
use sha2::{Digest, Sha256};

use mountmate_core::update_manifest::{
    CANONICAL_ASSET_NAMES, MANIFEST_ASSET_NAME, SIGNATURE_ASSET_NAME,
};

fn signing_tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_update-signing"))
}

fn write_release_assets(directory: &Path) {
    for (index, name) in CANONICAL_ASSET_NAMES.iter().enumerate() {
        std::fs::write(directory.join(name), format!("test zip payload {index}")).unwrap();
    }
}

fn digest_file(path: &Path) -> String {
    format!("sha256:{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

#[test]
fn exercise_local_signs_and_verifies_all_six_assets_with_an_ephemeral_key() {
    let temp = tempfile::tempdir().unwrap();
    write_release_assets(temp.path());

    let output = signing_tool()
        .args([
            "exercise-local",
            temp.path().to_str().unwrap(),
            "0.4.1-alpha.1",
            "prerelease",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "ephemeral signed release-set verification passed\n"
    );
}

#[test]
fn published_rest_metadata_must_match_signed_files_and_all_six_assets() {
    let temp = tempfile::tempdir().unwrap();
    write_release_assets(temp.path());
    let public_keys = temp.path().join("keys.json");
    let generated = signing_tool()
        .args(["generate", public_keys.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let private_key = generated.stdout;
    let key_registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&public_keys).unwrap()).unwrap();
    let key_id = key_registry["keys"][0]["key_id"].as_str().unwrap();
    let manifest = temp.path().join(MANIFEST_ASSET_NAME);
    let signature = temp.path().join(SIGNATURE_ASSET_NAME);

    let status = signing_tool()
        .args([
            "manifest",
            temp.path().to_str().unwrap(),
            "0.4.1-alpha.1",
            "prerelease",
            key_id,
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let mut child = signing_tool()
        .args([
            "sign",
            manifest.to_str().unwrap(),
            signature.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&private_key).unwrap();
    assert!(child.wait().unwrap().success());

    let mut assets = CANONICAL_ASSET_NAMES
        .iter()
        .map(|name| {
            let path = temp.path().join(name);
            json!({
                "name": name,
                "size": std::fs::metadata(&path).unwrap().len(),
                "digest": digest_file(&path),
                "browser_download_url": format!(
                    "https://github.com/Stardust0831/ssh-mountmate/releases/download/v0.4.1-alpha.1/{name}"
                )
            })
        })
        .collect::<Vec<_>>();
    for (name, path) in [
        (MANIFEST_ASSET_NAME, manifest.as_path()),
        (SIGNATURE_ASSET_NAME, signature.as_path()),
    ] {
        assets.push(json!({
            "name": name,
            "size": std::fs::metadata(path).unwrap().len(),
            "digest": digest_file(path),
            "browser_download_url": format!(
                "https://github.com/Stardust0831/ssh-mountmate/releases/download/v0.4.1-alpha.1/{name}"
            )
        }));
    }
    let metadata = temp.path().join("release.json");
    std::fs::write(
        &metadata,
        serde_json::to_vec(&json!({
            "tag_name": "v0.4.1-alpha.1",
            "prerelease": true,
            "assets": assets,
        }))
        .unwrap(),
    )
    .unwrap();

    let valid = signing_tool()
        .args([
            "verify-published",
            manifest.to_str().unwrap(),
            signature.to_str().unwrap(),
            public_keys.to_str().unwrap(),
            metadata.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );

    let mut replaced: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
    replaced["assets"][0]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    std::fs::write(&metadata, serde_json::to_vec(&replaced).unwrap()).unwrap();
    let invalid = signing_tool()
        .args([
            "verify-published",
            manifest.to_str().unwrap(),
            signature.to_str().unwrap(),
            public_keys.to_str().unwrap(),
            metadata.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("does not match"));
}

#[test]
fn signing_rejects_unbounded_private_key_input() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("manifest.json");
    let signature = temp.path().join("manifest.sig");
    std::fs::write(&manifest, b"{}").unwrap();
    let mut child = signing_tool()
        .args([
            "sign",
            manifest.to_str().unwrap(),
            signature.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 8 * 1024 + 1])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("private key input is too large"));
    assert!(!signature.exists());
}
