// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_optional_queries_and_transforms() {
    let root = temp_dir("emit_exe_std_optional_queries_and_transforms");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::option;
using std::process;

fun double(value: ?i32) ?i32 {
    value.map(\value: i32 -> value * 2)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let present = double(?21);
    if not present.isPresent() {
        return process::ExitCode(1)!;
    }
    if present is ?value {
        if value != 42 { return process::ExitCode(2)!; }
    }
    if double(null).isNull() == false or double(null).map(\value: i32 -> value).isPresent() {
        return process::ExitCode(3)!;
    }
    if double(?1).andThen(\value: i32 -> if value == 2 { ?41 } else { null }) is ?value {
        if value != 41 { return process::ExitCode(4)!; }
    } else {
        return process::ExitCode(5)!;
    }
    !()
}
"#,
    )
    .expect("write optional transform source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- optional transforms");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted optional transform executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_optionals_compare_by_case_and_value() {
    let root = temp_dir("emit_exe_std_optionals_compare_by_case_and_value");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    // No `using std::option;`: the implementation applies wherever `Eq` is
    // visible.
    std::fs::write(
        &main,
        r#"
using std::process;

enum Color {
    Red,
    Green,
}

fun find(present: bool) ?usize {
    if present { ?2 } else { null }
}

fun check(condition: bool, code: i32) process::ExitCode!() {
    if not condition {
        return process::ExitCode(code)!;
    }
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    check(find(true) == ?2, 1).?;
    check(find(true) != ?3, 2).?;
    check(find(false) != ?2, 3).?;
    check(find(false) == find(false), 4).?;
    check(find(false) == null, 5).?;
    check(null != find(true), 6).?;
    check(?2 == find(true), 7).?;
    let nested: ??u8 = ?null;
    check(nested != null, 8).?;
    check(nested == ?null, 9).?;
    check(nested != ?(?1), 10).?;
    let color: ?Color = ?.Green;
    check(color == ?.Green, 11).?;
    check(color != ?.Red, 12).?;
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
        .output_timeout_for_build("run nia emit --exe optional equality");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn check_rejects_optional_equality_without_value_equality() {
    let root = temp_dir("check_rejects_optional_equality_without_value_equality");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

fun same(a: ?Point, b: ?Point) bool {
    a == b
}

fun main() i32 {
    if same(null, null) { 0 } else { 1 }
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("run nia check on optional equality");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("?Point: Eq[?Point]")
            && stderr.contains("one of its required bounds is not satisfied"),
        "stderr:\n{stderr}"
    );
}
