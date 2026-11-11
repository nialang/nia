// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt::Write, fs, process::Command};

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir};

#[test]
fn wide_shift_runtime_matches_every_limb_boundary_in_all_pipelines() {
    let root = test_dir("wide_shift_runtime");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let values = [
        0,
        1,
        u128::MAX,
        1 << 127,
        (1 << 64) - 1,
        0x1234_5678_9abc_def0_fedc_ba98_7654_3210,
        0xfedc_ba98_7654_3210_1234_5678_9abc_def0,
    ];
    let counts = [
        0, 1, 31, 32, 33, 63, 64, 65, 95, 96, 97, 126, 127, 128, 129, 255,
    ];
    let mut source = format!(
        r#"
using std::process;

extern fun __ashlti3(value: u128, count: i32) u128;
extern fun __lshrti3(value: u128, count: i32) u128;
extern fun __ashrti3(value: i128, count: i32) i128;

pub fun main(init: process::Init) process::ExitCode!() {{
    _ = init;
    let cases: [(u128, i32, u128, u128, i128); {}] = [
"#,
        values.len() * counts.len()
    );
    for value in values {
        for count in counts {
            let (left, right, signed) = if count >= 128 {
                (0, 0, if (value >> 127) != 0 { -1 } else { 0 })
            } else {
                (value << count, value >> count, (value as i128) >> count)
            };
            writeln!(
                source,
                "        ({value}u128, {count}i32, {}u128, {}u128, {}i128),",
                left, right, signed,
            )
            .unwrap();
        }
    }
    source.push_str(
        r#"    ];
    for &(value, count, left, right, signed) in &cases[..] {
        if __ashlti3(value, count) != left {
            return process::ExitCode(1)!;
        }
        if __lshrti3(value, count) != right {
            return process::ExitCode(2)!;
        }
        if __ashrti3(value as i128, count) != signed {
            return process::ExitCode(3)!;
        }
    }
    !()
}
"#,
    );
    fs::write(&main, source).expect("write wide shift source");
    for options in [
        &["-O0", "--lto", "off"][..],
        &["-O1", "--lto", "off"],
        &["-O2", "--lto", "off"],
        &["-O3", "--lto", "off"],
        &["-Os", "--lto", "off"],
        &["-Oz", "--lto", "off"],
        &["-Oz", "--lto", "thin"],
        &["-Oz", "--lto", "full"],
    ] {
        let output = support::nia_command()
            .args(["emit", "--exe"])
            .args(options)
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit wide shift runtime executable");
        assert!(
            output.status.success(),
            "{options:?}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(&exe).status_timeout("run wide shift runtime executable");
        assert_eq!(run.code(), Some(0), "{options:?}");
    }
}
