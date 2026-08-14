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

static mut invocationCount: i32 = 0;
static mut total: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    (1..5).iter().forEach(&\[offset] value: i32 -> {
        invocationCount += 1;
        total += value + offset;
    });
    (1..3).iter().forEach(&\value: i32 -> {
        invocationCount += 1;
        total += value;
    });
    let mut values: [i32; 3] = [1, 2, 3];
    (&mut values).iterMut().forEach(&\value: &mut i32 -> {
        value.* += 1;
    });
    if invocationCount != 6 or total != 17
        or values[0] != 2 or values[1] != 3 or values[2] != 4
    {
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
    let total = (1..5).iter().fold(0, &\[offset] acc: i32, value: i32 -> {
        acc + value + offset
    });
    let summary = (1..5).iter().fold((0, 0), &\state: (i32, i32), value: i32 -> {
        (state.0 + value, state.1 + 1)
    });
    let prefix = (1..6).iter().take(3).fold(0, &\acc: i32, value: i32 -> {
        acc + value
    });
    if total != 14 or summary.0 != 10 or summary.1 != 4 or prefix != 6 {
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
fn emit_exe_std_iterator_try_fold_preserves_error_and_stops_consuming() {
    let root = temp_dir("emit_exe_std_iterator_try_fold_preserves_error_and_stops_consuming");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::process;

enum FoldError: i32 {
    Rejected = 7,
    _,
}

extend FoldError : error::IntoError[process::ExitCode] {
    const fn into_error(self) process::ExitCode {
        switch self {
            FoldError::Rejected => {},
            _ => {},
        }
        process::exit(9)
    }
}

static mut invocationCount: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    let folded: FoldError!(i32, i32) = (1..5).iter().tryFold(
        (0, 0),
        &\[offset] state: (i32, i32), value: i32 -> {
            invocationCount += 1;
            !(state.0 + value + offset, state.1 + 1)
        },
    );
    let summary = folded.?;
    if summary.0 != 14 or summary.1 != 4 or invocationCount != 4 {
        return process::exit(1)!;
    }

    invocationCount = 0;
    let failed = (1..6).iter().tryFold(
        0,
        &\sum: i32, value: i32 -> {
            invocationCount += 1;
            if value == 3 {
                FoldError::Rejected!
            } else {
                !(sum + value)
            }
        },
    );
    switch failed {
        !value => {
            _ = value;
            return process::exit(2)!;
        },
        FoldError::Rejected! => {},
        error! => {
            _ = error;
            return process::exit(3)!;
        },
    }
    if invocationCount != 3 {
        return process::exit(4)!;
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
        .output_timeout_for_build("run nia emit --exe std iterator tryFold");

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

static mut predicateCount: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let target = 4;
    let index = (1..6).iter().position(&\[target] value: i32 -> {
        predicateCount += 1;
        value == target
    });
    switch index {
        ?found => if found != 3 {
            return process::exit(1)!;
        },
        null => return process::exit(2)!,
    }
    let missing = (1..4).iter().position(&\value: i32 -> {
        predicateCount += 1;
        value == 9
    });
    switch missing {
        ?_ => return process::exit(3)!,
        null => {},
    }
    if predicateCount != 7 {
        return process::exit(4)!;
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
