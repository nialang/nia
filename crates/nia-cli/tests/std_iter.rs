// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_iterator_for_each_accepts_borrowed_closure() {
    let root = temp_dir("emit_exe_std_iterator_for_each_accepts_borrowed_closure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    (1..5).iter().forEach(&[offset](value: i32) () {
        _ = value;
        _ = offset;
    });
    !()
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
        .output_timeout_for_build("run nia emit --exe std iterator forEach");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_iterator_fold_accepts_capturing_borrowed_closure() {
    let root = temp_dir("emit_exe_std_iterator_fold_accepts_capturing_borrowed_closure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    let total = (1..5).iter().fold(0, &[offset](acc: i32, value: i32) i32 {
        acc + value + offset
    });
    let summary = (1..5).iter().fold((0, 0), &[](state: (i32, i32), value: i32) (i32, i32) {
        (state.0 + value, state.1 + 1)
    });
    if total != 14 or summary.0 != 10 or summary.1 != 4 {
        return process::exit(1)!;
    }
    !()
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
        .output_timeout_for_build("run nia emit --exe std iterator fold");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_iterator_position_consumes_items_with_borrowed_predicate() {
    let root = temp_dir("emit_exe_std_iterator_position_consumes_items_with_borrowed_predicate");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let target = 4;
    let index = (1..6).iter().position(&[target](value: i32) bool {
        value == target
    });
    switch index {
        ?found => if found != 3 {
            return process::exit(1)!;
        },
        null => return process::exit(2)!,
    }
    let missing = (1..4).iter().position(&[](value: i32) bool {
        value == 9
    });
    switch missing {
        ?_ => return process::exit(3)!,
        null => {},
    }
    !()
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
        .output_timeout_for_build("run nia emit --exe std iterator position");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}
