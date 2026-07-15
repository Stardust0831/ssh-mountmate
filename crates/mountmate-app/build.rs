use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("windows/SSHMountMate.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=SSHMountMate=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=SSHMountMate=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
