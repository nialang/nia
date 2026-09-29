// SPDX-License-Identifier: GPL-3.0-or-later
//! Every maintained target compiles from every maintained host.

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

/// Object-file identity checked from raw bytes, so no platform tool is needed.
fn object_format(bytes: &[u8]) -> Option<(&'static str, u16)> {
    match bytes {
        // ELF: `e_machine` at offset 18, little-endian.
        [0x7f, b'E', b'L', b'F', ..] if bytes.len() > 19 => {
            Some(("elf", u16::from_le_bytes([bytes[18], bytes[19]])))
        }
        // 64-bit little-endian Mach-O: `cputype` follows the magic.
        [0xcf, 0xfa, 0xed, 0xfe, cpu0, cpu1, ..] => {
            Some(("mach-o", u16::from_le_bytes([*cpu0, *cpu1])))
        }
        // COFF object: `Machine` is the first field.
        [machine0, machine1, ..] => Some(("coff", u16::from_le_bytes([*machine0, *machine1]))),
        _ => None,
    }
}

#[test]
fn every_maintained_target_emits_its_native_object_format() {
    let root = temp_dir("cross_target_objects");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        "fn add(a: i32, b: i32) i32 { a + b }\npub fn main() () { _ = add(1, 2); }\n",
    )
    .expect("write source");
    // Machine codes: ELF x86-64 62, i386 3, AArch64 183; Mach-O x86_64 7 and
    // arm64 12 (low half of the CPU type); COFF AMD64 0x8664.
    for (target, expected) in [
        ("x86_64-unknown-linux", ("elf", 62)),
        ("x86-unknown-linux", ("elf", 3)),
        ("aarch64-unknown-linux", ("elf", 183)),
        ("x86_64-pc-windows-msvc", ("coff", 0x8664)),
        ("x86_64-apple-macos", ("mach-o", 7)),
        ("aarch64-apple-macos", ("mach-o", 12)),
    ] {
        let object = root.join(format!("{target}.o"));
        let output = support::nia_command()
            .args(["--target", target, "emit", "--obj"])
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output_timeout_for_compiler("emit cross-target object");
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(&object).expect("read object");
        assert_eq!(object_format(&bytes), Some(expected), "{target}");
    }
}

#[test]
fn unsupported_target_names_are_diagnosed_with_the_matrix() {
    let output = support::nia_command()
        .args(["--target", "x86_64-unknown-linux-gnu", "check", "main.nia"])
        .output_timeout_for_compiler("reject unsupported target");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("unsupported target `x86_64-unknown-linux-gnu`"),
        "{stderr}"
    );
    assert!(stderr.contains("`aarch64-apple-macos`"), "{stderr}");
}
