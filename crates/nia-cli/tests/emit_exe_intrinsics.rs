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

    let values: u8x16 = @insert(@insert(@insert(@splat[u8x16](0u8), 1usize, 7u8), 4usize, 7u8), 15usize, 7u8);
    let mask = @bitmask(values == @splat[u8x16](7u8));
    if mask != 0x8012usize {
        return (1 as process::ExitCode)!;
    }

    let other = @bitmask(values == @splat[u8x16](0u8));
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

    if @ctz[usize](0usize) != 64usize {
        return (1 as process::ExitCode)!;
    }
    if @clz[usize](0usize) != 64usize {
        return (2 as process::ExitCode)!;
    }
    if @ctz[usize](0x8010usize) != 4usize {
        return (3 as process::ExitCode)!;
    }
    if @clz[usize](0x8010usize) != 48usize {
        return (4 as process::ExitCode)!;
    }
    if @popcount[usize](0x8010usize) != 2usize {
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
    let vec = @load_unaligned[u8x8](&bytes[1]);
    if @extract(vec, 0usize) != 1u8 {
        return (1 as process::ExitCode)!;
    }
    if @extract(vec, 7usize) != 8u8 {
        return (2 as process::ExitCode)!;
    }
    let mask = @bitmask(vec == @splat[u8x8](4u8));
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
