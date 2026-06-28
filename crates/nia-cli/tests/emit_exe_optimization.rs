// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emitted_executable_preserves_semantics_at_o0() {
    emitted_executable_preserves_semantics_at_optimization_level("-O0");
}

#[test]
fn emitted_executable_preserves_semantics_at_o1() {
    emitted_executable_preserves_semantics_at_optimization_level("-O1");
}

#[test]
fn emitted_executable_preserves_semantics_at_o2() {
    emitted_executable_preserves_semantics_at_optimization_level("-O2");
}

#[test]
fn emitted_executable_preserves_semantics_at_o3() {
    emitted_executable_preserves_semantics_at_optimization_level("-O3");
}

#[test]
fn emitted_executable_preserves_semantics_at_os() {
    emitted_executable_preserves_semantics_at_optimization_level("-Os");
}

#[test]
fn emitted_executable_preserves_semantics_at_oz() {
    emitted_executable_preserves_semantics_at_optimization_level("-Oz");
}

#[test]
fn emitted_executable_preserves_semantics_with_bare_o_alias() {
    emitted_executable_preserves_semantics_at_optimization_level("-O");
}

fn emitted_executable_preserves_semantics_at_optimization_level(level: &str) {
    let root = temp_dir(&format!(
        "emitted_executable_preserves_semantics_at_{}",
        level.trim_start_matches('-')
    ));
    let main = root.join("main.nia");
    let exe_name = format!(
        "main_{}{}",
        level.trim_start_matches('-'),
        std::env::consts::EXE_SUFFIX
    );
    let exe = root.join(exe_name);
    std::fs::write(
        &main,
        r#"
using std::process;

fn pick(flag: bool, a: i32, b: i32) i32 {
    if flag {
        a
    } else {
        b
    }
}

fn answer() i32 {
    40
}

fn identity[T](value: T) T {
    value
}

fn plus_two(value: i32) i32 {
    value + 2
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut x = answer();
    let mut y = x;
    y = identity[i32](y);
    let mut unused = plus_two(99);
    (pick(true, plus_two(y), unused) as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output_context = format!("run nia {level} emit --exe");
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg(level)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout(&output_context);

    assert!(
        output.status.success(),
        "{level} stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run_context = format!("run emitted executable for {level}");
    let status = Command::new(&exe).status_timeout(&run_context);
    assert_eq!(status.code(), Some(42), "{level}");
}
