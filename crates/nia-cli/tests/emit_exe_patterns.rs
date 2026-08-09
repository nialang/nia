use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_switch_destructuring_matches_const_semantics() {
    let root = temp_dir("emit_exe_switch_destructuring_matches_const_semantics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fn read_optional(value: ?i32) i32 {
    switch value {
        ?payload => payload,
        null => 0,
    }
}

fn read_error(value: i32!i32) i32 {
    switch value {
        !payload => payload,
        error! => error,
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if read_optional(?4) != 4 {
        return process::exit(1)!;
    }
    if read_optional(null) != 0 {
        return process::exit(2)!;
    }
    if read_error(!7) != 7 {
        return process::exit(3)!;
    }
    if read_error(5!) != 5 {
        return process::exit(4)!;
    }
    !{}
}
"#,
    )
    .expect("write switch destructuring source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit switch destructuring executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run switch destructuring executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_error_propagation_converts_with_into_error() {
    let root = temp_dir("emit_exe_error_propagation_converts_with_into_error");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::process;

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    Unknown = 3,
    _,
}

struct EmptySourceError {}
struct EmptyTargetError {}

static mut conversionCount: i32 = 0;

extend SourceError : error::IntoError[TargetError] {
    fn into_error(self) TargetError {
        conversionCount += 1;
        switch self {
            SourceError::Failed => TargetError::Converted,
            _ => TargetError::Unknown,
        }
    }
}

extend EmptySourceError : error::IntoError[EmptyTargetError] {
    fn into_error(self) EmptyTargetError {
        conversionCount += 1;
        {}
    }
}

fn failVoid() SourceError!void {
    SourceError::Failed!
}

fn failEmpty() EmptySourceError!void {
    EmptySourceError {}!
}

fn propagateWithDefer() TargetError!void {
    defer conversionCount *= 10;
    failVoid().?;
    !{}
}

fn propagateEmpty() EmptyTargetError!void {
    failEmpty().?;
    !{}
}

fn source(succeed: bool) SourceError!i32 {
    if succeed {
        !41
    } else {
        SourceError::Failed!
    }
}

fn propagate[Source, Target](value: Source!i32) Target!i32
where Source: error::IntoError[Target]
{
    !(value.? + 1)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch propagate[SourceError, TargetError](source(true)) {
        !value => {
            if value != 42 {
                return process::exit(1)!;
            }
        },
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }
    if conversionCount != 0 {
        return process::exit(5)!;
    }
    switch propagate[SourceError, TargetError](source(false)) {
        !value => {
            _ = value;
            return process::exit(3)!;
        },
        TargetError::Converted! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }
    if conversionCount != 1 {
        return process::exit(6)!;
    }
    conversionCount = 0;
    switch propagateWithDefer() {
        !ok => {
            _ = ok;
            return process::exit(7)!;
        },
        TargetError::Converted! => {},
        error! => {
            _ = error;
            return process::exit(8)!;
        },
    }
    if conversionCount != 10 {
        return process::exit(9)!;
    }
    conversionCount = 0;
    switch propagateEmpty() {
        !ok => {
            _ = ok;
            return process::exit(10)!;
        },
        error! => { _ = error; },
    }
    if conversionCount != 1 {
        return process::exit(11)!;
    }
    !{}
}
"#,
    )
    .expect("write IntoError propagation source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit IntoError propagation executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run IntoError propagation executable");
    assert_eq!(status.code(), Some(0));
}
