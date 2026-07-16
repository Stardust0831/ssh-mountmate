use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;

use mountmate_core::update_manifest::{
    MANIFEST_ASSET_NAME, MAX_KEY_REGISTRY_BYTES, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES,
    PublishedAsset, ReleaseChannel, SIGNATURE_ASSET_NAME, TrustedUpdateKeySet,
    build_release_manifest, generate_signing_key, sign_manifest, verify_local_release_set,
    verify_release_manifest,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAX_PRIVATE_KEY_INPUT_BYTES: u64 = 8 * 1024;
const MAX_RELEASE_METADATA_BYTES: usize = 2 * 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(help());
    };
    match command {
        "generate" if arguments.len() == 2 => generate(Path::new(&arguments[1])),
        "manifest" if arguments.len() == 6 => create_manifest(
            Path::new(&arguments[1]),
            &arguments[2],
            &arguments[3],
            &arguments[4],
            Path::new(&arguments[5]),
        ),
        "sign" if arguments.len() == 3 => sign(Path::new(&arguments[1]), Path::new(&arguments[2])),
        "verify-local" if arguments.len() == 5 => verify_local(
            Path::new(&arguments[1]),
            Path::new(&arguments[2]),
            Path::new(&arguments[3]),
            Path::new(&arguments[4]),
        ),
        "exercise-local" if arguments.len() == 4 => {
            exercise_local(Path::new(&arguments[1]), &arguments[2], &arguments[3])
        }
        "verify-published" if arguments.len() == 5 => verify_published(
            Path::new(&arguments[1]),
            Path::new(&arguments[2]),
            Path::new(&arguments[3]),
            Path::new(&arguments[4]),
        ),
        _ => Err(help()),
    }
}

fn exercise_local(assets_directory: &Path, version: &str, channel: &str) -> Result<(), String> {
    let channel = parse_channel(channel)?;
    let (private, public) = generate_signing_key().map_err(|error| error.to_string())?;
    let manifest = build_release_manifest(assets_directory, version, channel, &public.key_id)
        .map_err(|error| error.to_string())?;
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    let signature = sign_manifest(&private, &manifest_bytes).map_err(|error| error.to_string())?;
    let keys = TrustedUpdateKeySet::new(vec![public]).map_err(|error| error.to_string())?;
    verify_local_release_set(
        &manifest_bytes,
        signature.as_bytes(),
        &keys,
        assets_directory,
    )
    .map_err(|error| error.to_string())?;
    println!("ephemeral signed release-set verification passed");
    Ok(())
}

fn generate(public_key_output: &Path) -> Result<(), String> {
    if io::stdout().is_terminal() {
        return Err(
            "refusing to print a private key to a terminal; pipe stdout directly to the protected secret store"
                .into(),
        );
    }
    let (private, public) = generate_signing_key().map_err(|error| error.to_string())?;
    let public_file = json!({
        "schema_version": 1,
        "keys": [public],
    });
    fs::write(
        public_key_output,
        serde_json::to_vec_pretty(&public_file).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("could not write public key registry: {error}"))?;
    io::stdout()
        .write_all(private.as_bytes())
        .map_err(|error| format!("could not send private key to secret store: {error}"))?;
    Ok(())
}

fn create_manifest(
    assets_directory: &Path,
    version: &str,
    channel: &str,
    key_id: &str,
    output: &Path,
) -> Result<(), String> {
    let channel = parse_channel(channel)?;
    let manifest = build_release_manifest(assets_directory, version, channel, key_id)
        .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    fs::write(output, bytes).map_err(|error| format!("could not write manifest: {error}"))
}

fn parse_channel(value: &str) -> Result<ReleaseChannel, String> {
    match value {
        "stable" => Ok(ReleaseChannel::Stable),
        "prerelease" => Ok(ReleaseChannel::Prerelease),
        _ => Err("channel must be stable or prerelease".into()),
    }
}

fn sign(manifest: &Path, signature_output: &Path) -> Result<(), String> {
    if io::stdin().is_terminal() {
        return Err("refusing to read a private key from a terminal; provide it on stdin".into());
    }
    let mut private = Zeroizing::new(String::new());
    io::stdin()
        .take(MAX_PRIVATE_KEY_INPUT_BYTES + 1)
        .read_to_string(&mut private)
        .map_err(|error| format!("could not read private key from stdin: {error}"))?;
    if private.len() as u64 > MAX_PRIVATE_KEY_INPUT_BYTES {
        return Err("private key input is too large".into());
    }
    let manifest = read_bounded(manifest, MAX_MANIFEST_BYTES, "manifest")?;
    let signature = sign_manifest(&private, &manifest).map_err(|error| error.to_string())?;
    fs::write(signature_output, signature)
        .map_err(|error| format!("could not write signature: {error}"))
}

fn verify_local(
    manifest: &Path,
    signature: &Path,
    trusted_keys: &Path,
    assets_directory: &Path,
) -> Result<(), String> {
    let manifest_bytes = read_bounded(manifest, MAX_MANIFEST_BYTES, "manifest")?;
    let signature_bytes = read_bounded(signature, MAX_SIGNATURE_BYTES, "signature")?;
    let keys = read_bounded(trusted_keys, MAX_KEY_REGISTRY_BYTES, "trusted key registry")?;
    let keys = TrustedUpdateKeySet::from_json(&keys).map_err(|error| error.to_string())?;
    let verified =
        verify_local_release_set(&manifest_bytes, &signature_bytes, &keys, assets_directory)
            .map_err(|error| error.to_string())?;
    println!(
        "verified manifest version={} key_id={} assets={}",
        verified.manifest().version,
        verified.manifest().key_id,
        verified.manifest().assets.len()
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct GithubReleaseView {
    tag_name: String,
    prerelease: bool,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    size: u64,
    digest: String,
    #[serde(rename = "browser_download_url")]
    url: String,
}

fn verify_published(
    manifest: &Path,
    signature: &Path,
    trusted_keys: &Path,
    release_json: &Path,
) -> Result<(), String> {
    let manifest_bytes = read_bounded(manifest, MAX_MANIFEST_BYTES, "manifest")?;
    let signature_bytes = read_bounded(signature, MAX_SIGNATURE_BYTES, "signature")?;
    let keys = read_bounded(trusted_keys, MAX_KEY_REGISTRY_BYTES, "trusted key registry")?;
    let keys = TrustedUpdateKeySet::from_json(&keys).map_err(|error| error.to_string())?;
    let release_bytes = read_bounded(
        release_json,
        MAX_RELEASE_METADATA_BYTES,
        "GitHub Release metadata",
    )?;
    let release: GithubReleaseView = serde_json::from_slice(&release_bytes)
        .map_err(|error| format!("GitHub Release metadata is invalid: {error}"))?;
    verify_published_auxiliary(
        &release.assets,
        &release.tag_name,
        MANIFEST_ASSET_NAME,
        manifest,
    )?;
    verify_published_auxiliary(
        &release.assets,
        &release.tag_name,
        SIGNATURE_ASSET_NAME,
        signature,
    )?;
    let assets = release
        .assets
        .into_iter()
        .filter(|asset| asset.name.starts_with("SSHMountMate-") && asset.name.ends_with(".zip"))
        .map(|asset| PublishedAsset {
            name: asset.name,
            size: asset.size,
            digest: asset.digest,
            url: asset.url,
        })
        .collect::<Vec<_>>();
    let verified = verify_release_manifest(
        &manifest_bytes,
        &signature_bytes,
        &keys,
        &release.tag_name,
        release.prerelease,
        &assets,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "verified published release version={} key_id={} assets={}",
        verified.manifest().version,
        verified.manifest().key_id,
        verified.manifest().assets.len()
    );
    Ok(())
}

fn verify_published_auxiliary(
    assets: &[GithubReleaseAsset],
    tag: &str,
    name: &str,
    local_path: &Path,
) -> Result<(), String> {
    let matches = assets
        .iter()
        .filter(|asset| asset.name == name)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!("GitHub Release must contain exactly one {name}"));
    }
    let published = matches[0];
    let maximum = if name == MANIFEST_ASSET_NAME {
        MAX_MANIFEST_BYTES
    } else {
        MAX_SIGNATURE_BYTES
    };
    let bytes = read_bounded(local_path, maximum, name)?;
    let expected_url =
        format!("https://github.com/Stardust0831/ssh-mountmate/releases/download/{tag}/{name}");
    if published.size != bytes.len() as u64
        || published.digest != format!("sha256:{:x}", Sha256::digest(&bytes))
        || published.url != expected_url
    {
        return Err(format!(
            "GitHub Release metadata does not match local {name}"
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path, maximum: usize, description: &str) -> Result<Vec<u8>, String> {
    let file =
        fs::File::open(path).map_err(|error| format!("could not read {description}: {error}"))?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {description}: {error}"))?;
    if bytes.len() > maximum {
        return Err(format!("{description} is too large"));
    }
    Ok(bytes)
}

fn help() -> String {
    "usage:\n  update-signing generate PUBLIC_KEYS_JSON > PRIVATE_KEY_PIPE\n  update-signing manifest ASSETS_DIR VERSION CHANNEL KEY_ID OUTPUT_JSON\n  update-signing sign MANIFEST_JSON OUTPUT_SIG < PRIVATE_KEY_PIPE\n  update-signing verify-local MANIFEST_JSON SIGNATURE PUBLIC_KEYS_JSON ASSETS_DIR\n  update-signing exercise-local ASSETS_DIR VERSION CHANNEL\n  update-signing verify-published MANIFEST_JSON SIGNATURE PUBLIC_KEYS_JSON RELEASE_JSON"
        .into()
}
