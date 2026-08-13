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
    switch success {
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
    switch failure {
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
    switch success {
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
    switch recovered {
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
    switch replaced {
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
    const fn into_error(self) TargetError {
        switch self {
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
const width: usize = switch success {
    !(left, right) => left + right,
    cause! => 0,
} + switch failure {
    !value => value.0 + value.1,
    TargetError::Wrapped! => 8,
    cause! => 0,
} + switch standardFailure {
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
