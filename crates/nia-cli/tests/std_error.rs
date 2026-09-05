// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_error_map_error_maps_only_failure_arm() {
    let root = temp_dir("emit_exe_std_error_map_error_maps_only_failure_arm");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::result;
using std::process;

enum SourceError: i32 {
    Missing = 1,
    _,
}

enum TargetError: i32 {
    Wrapped = 2,
    Unexpected = 3,
    _,
}

fn source(ok: bool) SourceError!(i32, i32) {
    if ok {
        !(40, 2)
    } else {
        SourceError::Missing!
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    let success = source(true).mapError(&\cause: SourceError -> {
        _ = cause;
        std::builtin::trap();
        TargetError::Wrapped
    });
    match success {
        !(left, right) => {
            if left + right != 42 {
                return process::exit(1)!;
            }
        },
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }

    let failure = source(false).mapError(&\[offset] cause: SourceError -> {
        if cause == SourceError::Missing and offset == 1 {
            TargetError::Wrapped
        } else {
            TargetError::Unexpected
        }
    });
    match failure {
        !value => {
            _ = value;
            return process::exit(3)!;
        },
        TargetError::Wrapped! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
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
        .output_timeout_for_build("run nia emit --exe std error mapError");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_error_or_else_recovers_or_replaces_failure() {
    let root = temp_dir("emit_exe_std_error_or_else_recovers_or_replaces_failure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::result;
using std::process;

enum SourceError: i32 {
    Missing = 1,
    _,
}

enum TargetError: i32 {
    Wrapped = 2,
    Unexpected = 3,
    _,
}

fn source(ok: bool) SourceError!(i32, i32) {
    if ok {
        !(20, 22)
    } else {
        SourceError::Missing!
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let success = source(true).orElse(&\cause: SourceError -> {
        _ = cause;
        std::builtin::trap();
        TargetError::Unexpected!
    });
    match success {
        !(left, right) => if left + right != 42 {
            return process::exit(1)!;
        },
        cause! => {
            _ = cause;
            return process::exit(2)!;
        },
    }

    let offset = 2;
    let recovered = source(false).orElse(&\[offset] cause: SourceError -> {
        if cause == SourceError::Missing {
            !(40, offset)
        } else {
            TargetError::Unexpected!
        }
    });
    match recovered {
        !(left, right) => if left + right != 42 {
            return process::exit(3)!;
        },
        cause! => {
            _ = cause;
            return process::exit(4)!;
        },
    }

    let replaced = source(false).orElse(&\cause: SourceError -> {
        if cause == SourceError::Missing {
            TargetError::Wrapped!
        } else {
            TargetError::Unexpected!
        }
    });
    match replaced {
        !value => {
            _ = value;
            return process::exit(5)!;
        },
        TargetError::Wrapped! => {},
        cause! => {
            _ = cause;
            return process::exit(6)!;
        },
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
        .output_timeout_for_build("run nia emit --exe std error orElse");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_error_map_and_then_transform_success_only() {
    let root = temp_dir("emit_exe_std_error_map_and_then_transform_success_only");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::result;
using std::process;

enum Failure: i32 {
    Bad = 1,
    _,
}

fn fallible(value: Failure!i32) Failure!i32 {
    value.map(&\value: i32 -> value + 1).andThen(&\value: i32 -> {
        if value == 3 { Failure::Bad! } else { !(value * 2) }
    })
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let success = fallible(!20);
    if not success.isSuccess() or success.isError() {
        return process::exit(1)!;
    }
    match success {
        !value => if value != 42 { return process::exit(1)!; },
        error! => return process::exit(2)!,
    }
    let failure = fallible(Failure::Bad!);
    if not failure.isError() or failure.isSuccess() {
        return process::exit(3)!;
    }
    match failure {
        Failure::Bad! => {},
        !value => { _ = value; return process::exit(3)!; },
        error! => { _ = error; return process::exit(4)!; },
    }
    !()
}
"#,
    )
    .expect("write error map/andThen source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- error map andThen");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted error map/andThen executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_error_inspect_error_observes_only_failures() {
    let root = temp_dir("emit_exe_std_error_inspect_error_observes_only_failures");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::result;

enum Failure: i32 {
    Missing = 1,
    _,
}

fn source(calls: &mut usize, succeeds: bool) Failure!usize {
    calls.* += 1usize;
    if succeeds { !42usize } else { Failure::Missing! }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut calls = 0usize;
    let mut observed = 0usize;

    let success = source(&mut calls, true).inspectError(&\[&mut observed] cause: Failure -> {
        _ = cause;
        observed.* += 1usize;
    });
    match success {
        !value => if value != 42usize {
            return process::exit(1)!;
        },
        error! => return process::exit(2)!,
    }
    if calls != 1usize or observed != 0usize {
        return process::exit(3)!;
    }

    let failure = source(&mut calls, false).inspectError(&\[&mut observed] cause: Failure -> {
        if cause == Failure::Missing {
            observed.* += 10usize;
        }
    });
    match failure {
        Failure::Missing! => {},
        !value => {
            _ = value;
            return process::exit(4)!;
        },
        error! => return process::exit(5)!,
    }
    if calls != 2usize or observed != 10usize {
        return process::exit(6)!;
    }
    !()
}
"#,
    )
    .expect("write inspectError source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- std error inspectError");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted inspectError executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_into_error_is_const_propagation_protocol() {
    let root = temp_dir("emit_exe_std_into_error_is_const_propagation_protocol");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::fs;
using std::process;

enum SourceError: i32 {
    Missing = 1,
    _,
}

enum TargetError: i32 {
    Wrapped = 2,
    Unexpected = 3,
    _,
}

extend SourceError : error::IntoError[TargetError] {
    const fn intoError(self) TargetError {
        match self {
            SourceError::Missing => TargetError::Wrapped,
            _ => TargetError::Unexpected,
        }
    }
}

const fn propagate(value: SourceError!(usize, usize)) TargetError!(usize, usize) {
    !(value.?)
}

const fn propagateStd(value: fs::Error!usize) process::ExitCode!usize {
    !(value.?)
}

const success = propagate(!(20, 22));
const failure = propagate(SourceError::Missing!);
const standardFailure = propagateStd(fs::Error::NotFound!);
const width: usize = match success {
    !(left, right) => left + right,
    cause! => 0,
} + match failure {
    !value => value.0 + value.1,
    TargetError::Wrapped! => 8,
    cause! => 0,
} + match standardFailure {
    !value => value,
    cause! => cause as i32 as usize,
};

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    if width != 52 {
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
        .output_timeout_for_build("run nia emit --exe const std IntoError propagation");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_cleanup_after_runs_cleanup_and_preserves_primary_failure() {
    let root = temp_dir("emit_exe_std_cleanup_after_runs_cleanup_and_preserves_primary_failure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::process;

enum Failure: i32 {
    Primary = 1,
    Cleanup = 2,
    _,
}

fn operation(order: &mut usize, succeeds: bool) Failure!usize {
    order.* = order.* * 10usize + 1usize;
    if succeeds { !7usize } else { Failure::Primary! }
}

fn cleanup(order: &mut usize, succeeds: bool) Failure!() {
    order.* = order.* * 10usize + 2usize;
    if succeeds { !() } else { Failure::Cleanup! }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut order = 0usize;
    match operation(&mut order, true).cleanupAfter(cleanup(&mut order, true)) {
        !value => { if value != 7usize { return process::exit(1)!; } },
        failure! => { _ = failure; return process::exit(2)!; },
    }
    if order != 12usize { return process::exit(3)!; }

    order = 0usize;
    match operation(&mut order, true).cleanupAfter(cleanup(&mut order, false)) {
        Failure::Cleanup! => {},
        !value => { _ = value; return process::exit(4)!; },
        failure! => { _ = failure; return process::exit(5)!; },
    }
    if order != 12usize { return process::exit(6)!; }

    order = 0usize;
    match operation(&mut order, false).cleanupAfter(cleanup(&mut order, true)) {
        Failure::Primary! => {},
        !value => { _ = value; return process::exit(7)!; },
        failure! => { _ = failure; return process::exit(8)!; },
    }
    if order != 12usize { return process::exit(9)!; }

    order = 0usize;
    match operation(&mut order, false).cleanupAfter(cleanup(&mut order, false)) {
        Failure::Primary! => {},
        !value => { _ = value; return process::exit(10)!; },
        failure! => { _ = failure; return process::exit(11)!; },
    }
    if order != 12usize { return process::exit(12)!; }
    !()
}
"#,
    )
    .expect("write cleanupAfter source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- cleanupAfter");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted cleanupAfter executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_cleanup_accumulator_runs_all_and_keeps_first_failure() {
    let root = temp_dir("emit_exe_std_cleanup_accumulator_runs_all_and_keeps_first_failure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::process;

enum Failure: i32 {
    First = 1,
    Second = 2,
    _,
}

fn first(counter: &mut usize) Failure!() {
    counter.* += 1usize;
    Failure::First!
}

fn second(counter: &mut usize) Failure!() {
    counter.* += 1usize;
    Failure::Second!
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut counter: usize = 0;
    let mut cleanup = error::cleanup::CleanupAccumulator[Failure]::init();
    cleanup.attempt(first(&mut counter));
    cleanup.attempt(second(&mut counter));
    if counter != 2usize or cleanup.isClean() {
        return process::exit(1)!;
    }
    match cleanup.finish() {
        Failure::First! => {},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
        !value => {
            _ = value;
            return process::exit(3)!;
        },
    }

    let mut success = error::CleanupAccumulator[Failure]::init();
    success.attempt(!());
    if not success.isClean() {
        return process::exit(4)!;
    }
    match success.finish() {
        !ok => {
            _ = ok;
        },
        error! => {
            _ = error;
            return process::exit(5)!;
        },
    }
    !()
}
"#,
    )
    .expect("write cleanup accumulator source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit -- cleanup accumulator");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted cleanup accumulator executable");
    assert_eq!(run.code(), Some(0));
}
