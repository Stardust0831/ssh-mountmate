use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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

        let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../assets/ssh-mountmate-logo.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("SSHMountMate.res");
        write_icon_resources(&icon, &resource);
        println!(
            "cargo:rustc-link-arg-bin=SSHMountMate={}",
            resource.display()
        );
    }
}

// MSVC link.exe and lld-link both accept standard Win32 .res files. Writing the
// resource records directly avoids depending on an SDK rc.exe search path when
// cross-compiling or using an ARM64 Windows runner. Resource data is independent
// of the target architecture; the linker creates the target-specific section.
fn write_icon_resources(icon: &Path, destination: &Path) {
    let ico = fs::read(icon).expect("could not read application ICO");
    assert!(
        ico.len() >= 6 && ico[..4] == [0, 0, 1, 0],
        "invalid ICO header"
    );
    let count = u16::from_le_bytes([ico[4], ico[5]]);
    assert!(count > 0, "application ICO has no images");
    assert!(
        ico.len() >= 6 + usize::from(count) * 16,
        "truncated ICO directory"
    );

    let mut resources = Vec::new();
    // Every 32-bit resource file starts with an empty, ordinal-zero record.
    append_resource(&mut resources, 0, 0, &[]);
    let mut group = ico[..6].to_vec();
    for index in 0..count {
        let start = 6 + usize::from(index) * 16;
        let entry = &ico[start..start + 16];
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let end = offset.checked_add(size).expect("ICO image range overflow");
        let data = ico.get(offset..end).expect("truncated ICO image");
        assert!(!data.is_empty(), "empty ICO image");
        let resource_id = index + 1;
        append_resource(&mut resources, 3, resource_id, data); // RT_ICON

        // GRPICONDIRENTRY uses a resource ID in place of the ICO file offset.
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&resource_id.to_le_bytes());
    }
    append_resource(&mut resources, 14, 1, &group); // RT_GROUP_ICON
    fs::write(destination, resources).expect("could not write application icon resources");
}

fn append_resource(output: &mut Vec<u8>, resource_type: u16, id: u16, data: &[u8]) {
    let size = u32::try_from(data.len()).expect("icon resource is too large");
    output.extend_from_slice(&size.to_le_bytes());
    output.extend_from_slice(&32_u32.to_le_bytes()); // HeaderSize with ordinal type/name
    output.extend_from_slice(&0xffff_u16.to_le_bytes());
    output.extend_from_slice(&resource_type.to_le_bytes());
    output.extend_from_slice(&0xffff_u16.to_le_bytes());
    output.extend_from_slice(&id.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes()); // DataVersion
    let flags: u16 = if resource_type == 0 { 0 } else { 0x1030 };
    output.extend_from_slice(&flags.to_le_bytes()); // Moveable, pure, discardable
    output.extend_from_slice(&0_u16.to_le_bytes()); // Neutral language
    output.extend_from_slice(&0_u32.to_le_bytes()); // Version
    output.extend_from_slice(&0_u32.to_le_bytes()); // Characteristics
    output.extend_from_slice(data);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
}
