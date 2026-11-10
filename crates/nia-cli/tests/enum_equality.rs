// SPDX-License-Identifier: GPL-3.0-or-later
//! Built-in equality on enums: an enum without payloads compares its
//! discriminant; an enum with payloads has no intrinsic `Eq`, because the
//! discriminant alone cannot say whether two values are equal.
use std::fs;
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn fieldless_enums_compare_in_every_module() {
    let root = temp_dir("fieldless_enums_compare_in_every_module");
    fs::write(
        root.join("kinds.nia"),
        "pub enum Kind { Null, Number }\npub fun number() Kind { .Number }\n",
    )
    .expect("write kinds");
    let main = root.join("main.nia");
    fs::write(
        &main,
        r#"using std::process;
using std::test;
module kinds;

enum Local { A, B }

fun same[T](left: T, right: T) bool
where T: Eq[T]
{
    left == right
}

fun check() test::Error!() {
    test::expectEqual(kinds::number(), kinds::Kind::Number).?;
    test::expect(same(Local::A, Local::A) and Local::A != Local::B).?;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if check() is error! {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write main");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit fieldless enum comparisons");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).status_timeout("run fieldless enum comparisons");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn enums_with_payloads_have_no_intrinsic_equality() {
    let root = temp_dir("enums_with_payloads_have_no_intrinsic_equality");
    fs::write(
        root.join("errors.nia"),
        "pub enum Error { Syntax(u32), InvalidState }\n",
    )
    .expect("write errors");
    let main = root.join("main.nia");
    fs::write(
        &main,
        r#"module errors;

enum Local { Value(u64), Empty }

fun same[T](left: T, right: T) bool
where T: Eq[T]
{
    left == right
}

fun main() i32 {
    let local = Local::Value(3);
    let remote = errors::Error::InvalidState;
    if local == Local::Empty or same(remote, errors::Error::InvalidState) { 1 } else { 0 }
}
"#,
    )
    .expect("write main");
    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_build("check payload enum comparisons");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("trait bound not satisfied: Local: Eq[Local]"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("trait bound not satisfied: Error: Eq[Error]"),
        "stderr:\n{stderr}"
    );
    assert!(!stderr.contains("internal"), "stderr:\n{stderr}");
}
