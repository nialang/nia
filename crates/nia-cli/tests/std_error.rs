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
    let success = source(true).mapError(&[](cause: SourceError) TargetError {
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

    let failure = source(false).mapError(&[offset](cause: SourceError) TargetError {
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
