use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

const PATH_ENV: &str = "SSH_MOUNTMATE_EMBED_RCLONE_PATH";
const SHA256_ENV: &str = "SSH_MOUNTMATE_EMBED_RCLONE_SHA256";

fn main() {
    println!("cargo:rerun-if-env-changed={PATH_ENV}");
    println!("cargo:rerun-if-env-changed={SHA256_ENV}");

    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is required")).join("embedded-rclone");
    let path = env::var_os(PATH_ENV).map(PathBuf::from);
    if let Some(path) = &path {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let expected = env::var(SHA256_ENV).ok();
    let digest = match (path, expected) {
        (None, None) => {
            fs::write(&output, []).expect("could not create the empty embedded-rclone payload");
            String::new()
        }
        (Some(path), Some(expected)) => {
            let expected = expected.trim().to_ascii_lowercase();
            assert!(
                expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{SHA256_ENV} must be a full SHA-256 digest"
            );
            let payload = fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "could not read embedded rclone at {}: {error}",
                    path.display()
                )
            });
            let actual = format!("{:x}", Sha256::digest(&payload));
            assert_eq!(
                actual, expected,
                "embedded rclone does not match {SHA256_ENV}"
            );
            fs::write(&output, payload).expect("could not stage embedded rclone for compilation");
            actual
        }
        _ => panic!("{PATH_ENV} and {SHA256_ENV} must be set together"),
    };

    println!(
        "cargo:rustc-env=SSH_MOUNTMATE_EMBEDDED_RCLONE_PATH={}",
        output.display()
    );
    println!("cargo:rustc-env=SSH_MOUNTMATE_EMBEDDED_RCLONE_SHA256={digest}");
}
