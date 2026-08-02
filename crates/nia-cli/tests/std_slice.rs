// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_slice_direct_borrowed_iteration() {
    let root = temp_dir("emit_exe_std_slice_direct_borrowed_iteration");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::slice;

fn sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

fn sumMutView(values: &mut [i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let values: [5]i32 = [2, 3, 5, 7, 11];
    if sum(&values) != 28 {
        return process::exit(1)!;
    }

    let mut middle = 0;
    for &value in &values[1..4] {
        middle += value;
    }
    if middle != 15 {
        return process::exit(2)!;
    }

    let mut writable: [3]i32 = [10, 20, 30];
    if sumMutView(&mut writable) != 60 {
        return process::exit(3)!;
    }
    for value in (&mut writable[..]).iterMut().rev() {
        value.* += 1;
    }
    if writable[0] != 11 or writable[1] != 21 or writable[2] != 31 {
        return process::exit(4)!;
    }

    let iter = (&values[..]).iter();
    if iter.len() != 5 or iter.isEmpty() {
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
