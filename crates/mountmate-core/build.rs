use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn main() {
    let rclone_digest = stage_payload(
        "SSH_MOUNTMATE_EMBED_RCLONE_PATH",
        "SSH_MOUNTMATE_EMBED_RCLONE_SHA256",
        "embedded-rclone",
        "SSH_MOUNTMATE_EMBEDDED_RCLONE_PATH",
        "rclone",
    );
    let plink_digest = stage_payload(
        "SSH_MOUNTMATE_EMBED_PLINK_PATH",
        "SSH_MOUNTMATE_EMBED_PLINK_SHA256",
        "embedded-plink.exe",
        "SSH_MOUNTMATE_EMBEDDED_PLINK_PATH",
        "Plink",
    );
    println!("cargo:rustc-env=SSH_MOUNTMATE_EMBEDDED_RCLONE_SHA256={rclone_digest}");
    println!("cargo:rustc-env=SSH_MOUNTMATE_EMBEDDED_PLINK_SHA256={plink_digest}");
}

fn stage_payload(
    path_env: &str,
    digest_env: &str,
    output_name: &str,
    output_env: &str,
    label: &str,
) -> String {
    println!("cargo:rerun-if-env-changed={path_env}");
    println!("cargo:rerun-if-env-changed={digest_env}");

    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is required")).join(output_name);
    let path = env::var_os(path_env).map(PathBuf::from);
    if let Some(path) = &path {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let expected = env::var(digest_env).ok();
    let digest = match (path, expected) {
        (None, None) => {
            fs::write(&output, []).expect("could not create the empty embedded payload");
            String::new()
        }
        (Some(path), Some(expected)) => {
            let expected = expected.trim().to_ascii_lowercase();
            assert!(
                expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{digest_env} must be a full SHA-256 digest"
            );
            let payload = fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "could not read embedded {label} at {}: {error}",
                    path.display()
                )
            });
            let actual = format!("{:x}", Sha256::digest(&payload));
            assert_eq!(
                actual, expected,
                "embedded {label} does not match {digest_env}"
            );
            fs::write(&output, payload).expect("could not stage embedded tool for compilation");
            actual
        }
        _ => panic!("{path_env} and {digest_env} must be set together"),
    };

    println!("cargo:rustc-env={output_env}={}", output.display());
    digest
}
