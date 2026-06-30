// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_simd_bitmask_matches_lane_bits() {
    let root = temp_dir("emit_exe_simd_bitmask_matches_lane_bits");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let values: u8x16 = std::builtin::insert(std::builtin::insert(std::builtin::insert(std::builtin::splat[u8x16](0u8), 1usize, 7u8), 4usize, 7u8), 15usize, 7u8);
    let mask = std::builtin::bitmask(values == std::builtin::splat[u8x16](7u8));
    if mask != 0x8012usize {
        return (1 as process::ExitCode)!;
    }

    let other = std::builtin::bitmask(values == std::builtin::splat[u8x16](0u8));
    if other != 0x7fedusize {
        return (2 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_bit_intrinsics_are_zero_defined() {
    let root = temp_dir("emit_exe_bit_intrinsics_are_zero_defined");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    if std::builtin::ctz[usize](0usize) != 64usize {
        return (1 as process::ExitCode)!;
    }
    if std::builtin::clz[usize](0usize) != 64usize {
        return (2 as process::ExitCode)!;
    }
    if std::builtin::ctz[usize](0x8010usize) != 4usize {
        return (3 as process::ExitCode)!;
    }
    if std::builtin::clz[usize](0x8010usize) != 48usize {
        return (4 as process::ExitCode)!;
    }
    if std::builtin::popcount[usize](0x8010usize) != 2usize {
        return (5 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_char_checks_unicode_scalar_values() {
    let root = temp_dir("emit_exe_char_checks_unicode_scalar_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::unicode;

fn generic_char[T](value: T) ?char
where T: Char {
    value.char()
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let ascii = if ?ch = 65u32.char() {
        ch
    } or null {
        return (1 as process::ExitCode)!;
    };
    if ascii.codepoint() != 65u32 {
        return (2 as process::ExitCode)!;
    }

    let generic_ascii = if ?ch = generic_char(66u32) {
        ch
    } or null {
        return (10 as process::ExitCode)!;
    };
    if generic_ascii.codepoint() != 66u32 {
        return (11 as process::ExitCode)!;
    }

    let max = if ?ch = [char]::from_u32(0x10ffffu32) {
        ch
    } or null {
        return (3 as process::ExitCode)!;
    };
    if max.codepoint() != 0x10ffffu32 {
        return (4 as process::ExitCode)!;
    }

    if ?ch = 0xd800u32.char() {
        _ = ch;
        return (5 as process::ExitCode)!;
    } or null {}
    if ?ch = 0x110000u32.char() {
        _ = ch;
        return (6 as process::ExitCode)!;
    } or null {}

    let euro_bytes: [3]u8 = [0xe2u8, 0x82u8, 0xacu8];
    let euro = if ?decoded = unicode::utf8_decode_first(&euro_bytes) {
        decoded
    } or null {
        return (7 as process::ExitCode)!;
    };
    if euro.len() != 3usize or euro.char().codepoint() != 0x20acu32 {
        return (8 as process::ExitCode)!;
    }

    let overlong: [2]u8 = [0xc0u8, 0x80u8];
    if ?decoded = unicode::utf8_decode_first(&overlong) {
        _ = decoded;
        return (9 as process::ExitCode)!;
    } or null {}

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_unaligned_vector_load_reads_lanes() {
    let root = temp_dir("emit_exe_unaligned_vector_load_reads_lanes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let bytes: [10]u8 = [99u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 100u8];
    let vec = std::builtin::load_unaligned[u8x8](&bytes[1]);
    if std::builtin::extract(vec, 0usize) != 1u8 {
        return (1 as process::ExitCode)!;
    }
    if std::builtin::extract(vec, 7usize) != 8u8 {
        return (2 as process::ExitCode)!;
    }
    let mask = std::builtin::bitmask(vec == std::builtin::splat[u8x8](4u8));
    if mask != 0x08usize {
        return (3 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
