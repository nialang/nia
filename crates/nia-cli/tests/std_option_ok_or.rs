// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

// Use fully-qualified enum variants here because the generic parameter does not
// identify which enum a shorthand variant belongs to.

#[test]
fn option_ok_or_converts_some_to_success() {
    let root = temp_dir("option_ok_or_converts_some_to_success");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let some: ?i32 = ?42;
    let result = some.okOr(mem::Error::Invalid);

    if result is !value and value == 42 {
        !()
    } else {
        process::ExitCode(1)!
    }
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
        .output_timeout_for_build("compile option okOr test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_for_runtime("run option okOr test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn option_ok_or_converts_null_to_error() {
    let root = temp_dir("option_ok_or_converts_null_to_error");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let none: ?i32 = null;
    let result = none.okOr(mem::Error::OutOfMemory);

    if result is mem::Error::OutOfMemory! {
        !()
    } else {
        process::ExitCode(1)!
    }
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
        .output_timeout_for_build("compile option okOr null test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_for_runtime("run option okOr null test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn option_ok_or_else_infers_error_from_closures() {
    let root = temp_dir("option_ok_or_else_infers_error_from_closures");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let some: ?i32 = ?17;
    let success = some.okOrElse(\-> mem::Error::Leak);
    if success is !value and value == 17 {
        // The supplier must not replace a present value.
    } else {
        return process::ExitCode(1)!;
    }

    let none: ?i32 = null;
    let result = none.okOrElse(\-> mem::Error::Leak);
    let blockResult = none.okOrElse(\-> {
        mem::Error::Leak
    });

    if result is mem::Error::Leak! and blockResult is mem::Error::Leak! {
        !()
    } else {
        process::ExitCode(1)!
    }
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
        .output_timeout_for_build("compile option okOrElse closure inference test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run =
        Command::new(&exe).output_timeout_for_runtime("run option okOrElse closure inference test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn option_ok_or_propagates_with_try() {
    let root = temp_dir("option_ok_or_propagates_with_try");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

fun compute(input: ?i32) mem::Error!i32 {
    let value = input.okOr(mem::Error::Invalid).?;
    !(value * 2)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    if compute(?21) is !value and value == 42 {
        // ok
    } else {
        return process::ExitCode(1)!;
    }

    if compute(null) is mem::Error::Invalid! {
        !()
    } else {
        process::ExitCode(2)!
    }
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
        .output_timeout_for_build("compile option okOr propagation test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_for_runtime("run option okOr propagation test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn option_ok_or_chains_with_map() {
    let root = temp_dir("option_ok_or_chains_with_map");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let some: ?i32 = ?10;
    let result = some
        .map(\x -> x * 2)
        .okOr(mem::Error::Invalid);

    if result is !value and value == 20 {
        !()
    } else {
        process::ExitCode(1)!
    }
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
        .output_timeout_for_build("compile option okOr chaining test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_for_runtime("run option okOr chaining test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn option_ok_or_works_with_different_error_types() {
    let root = temp_dir("option_ok_or_works_with_different_error_types");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

enum CustomError: i32 {
    NotFound = 1,
    InvalidInput = 2,
    _,
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let none: ?i32 = null;
    let result = none.okOr(CustomError::NotFound);

    if result is CustomError::NotFound! {
        !()
    } else {
        process::ExitCode(1)!
    }
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
        .output_timeout_for_build("compile option okOr custom error test");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_for_runtime("run option okOr custom error test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
