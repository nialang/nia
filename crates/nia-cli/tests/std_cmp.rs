// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_cmp_max_is_generic() {
    let root = temp_dir("emit_exe_std_cmp_max_is_generic");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::cmp;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    if cmp::max(3i32, 7i32) != 7i32 {
        return process::exit(1)!;
    }
    if cmp::max(12usize, 5usize) != 12usize {
        return process::exit(2)!;
    }
    if cmp::max(9u64, 9u64) != 9u64 {
        return process::exit(3)!;
    }
    !()
}
"#,
    )
    .expect("write cmp max source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- cmp max");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted cmp max executable");
    assert_eq!(run.code(), Some(0));
}
