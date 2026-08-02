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
        return process::exit(1)!;
    }

    let other = std::builtin::bitmask(values == std::builtin::splat[u8x16](0u8));
    if other != 0x7fedusize {
        return process::exit(2)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

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
        return process::exit(1)!;
    }
    if std::builtin::clz[usize](0usize) != 64usize {
        return process::exit(2)!;
    }
    if std::builtin::ctz[usize](0x8010usize) != 4usize {
        return process::exit(3)!;
    }
    if std::builtin::clz[usize](0x8010usize) != 48usize {
        return process::exit(4)!;
    }
    if std::builtin::popcount[usize](0x8010usize) != 2usize {
        return process::exit(5)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

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

    let ascii = switch 65u32.char() {
        ?ch => {
            ch
        },
        null => {
            return process::exit(1)!;
        },
    };
    if ascii.codepoint() != 65u32 {
        return process::exit(2)!;
    }

    let generic_ascii = switch generic_char(66u32) {
        ?ch => {
            ch
        },
        null => {
            return process::exit(10)!;
        },
    };
    if generic_ascii.codepoint() != 66u32 {
        return process::exit(11)!;
    }

    let max = switch [char]::from_u32(0x10ffffu32) {
        ?ch => {
            ch
        },
        null => {
            return process::exit(3)!;
        },
    };
    if max.codepoint() != 0x10ffffu32 {
        return process::exit(4)!;
    }

    switch 0xd800u32.char() {
        ?ch => {
            _ = ch;
            return process::exit(5)!;
        },
        null => {},
    }
    switch 0x110000u32.char() {
        ?ch => {
            _ = ch;
            return process::exit(6)!;
        },
        null => {},
    }

    let euro_bytes: [3]u8 = [0xe2u8, 0x82u8, 0xacu8];
    let euro = switch unicode::decodeUtf8First(&euro_bytes) {
        !decoded => {
            decoded
        },
        error! => {
            _ = error;
            return process::exit(7)!;
        },
    };
    if euro.len() != 3usize or euro.char().codepoint() != 0x20acu32 {
        return process::exit(8)!;
    }

    let overlong: [2]u8 = [0xc0u8, 0x80u8];
    switch unicode::decodeUtf8First(&overlong) {
        !decoded => { _ = decoded; return process::exit(9)!; },
        error! => if error != unicode::Utf8DecodeError::Overlong {
            return process::exit(12)!;
        },
    }

    switch unicode::decodeUtf8First(&overlong[0..0]) {
        !decoded => { _ = decoded; return process::exit(13)!; },
        error! => if error != unicode::Utf8DecodeError::Empty {
            return process::exit(14)!;
        },
    }

    let truncated: [2]u8 = [0xe2u8, 0x82u8];
    switch unicode::decodeUtf8First(&truncated) {
        !decoded => { _ = decoded; return process::exit(15)!; },
        error! => if error != unicode::Utf8DecodeError::Truncated {
            return process::exit(16)!;
        },
    }

    let invalid_leading: [1]u8 = [0x80u8];
    switch unicode::decodeUtf8First(&invalid_leading) {
        !decoded => { _ = decoded; return process::exit(17)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidLeadingByte {
            return process::exit(18)!;
        },
    }

    let invalid_continuation: [3]u8 = [0xe2u8, 0x28u8, 0xa1u8];
    switch unicode::decodeUtf8First(&invalid_continuation) {
        !decoded => { _ = decoded; return process::exit(19)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidContinuation {
            return process::exit(20)!;
        },
    }

    let invalid_scalar: [3]u8 = [0xedu8, 0xa0u8, 0x80u8];
    switch unicode::decodeUtf8First(&invalid_scalar) {
        !decoded => { _ = decoded; return process::exit(21)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidScalar {
            return process::exit(22)!;
        },
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

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
        return process::exit(1)!;
    }
    if std::builtin::extract(vec, 7usize) != 8u8 {
        return process::exit(2)!;
    }
    let mask = std::builtin::bitmask(vec == std::builtin::splat[u8x8](4u8));
    if mask != 0x08usize {
        return process::exit(3)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
