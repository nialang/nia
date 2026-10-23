// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn generic_methods_infer_and_execute_capturing_callable_arguments() {
    let root = temp_dir("generic_methods_infer_and_execute_capturing_callable_arguments");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Source { value: i32 }
struct Target { value: i32 }
struct Runner {}

fun makeSource() Source { Source { value: 20 } }

extend Source {
    fun intoTarget(self) Target { Target { value: self.value * 2 } }
}

extend Runner {
    fun apply[Input, Output, Mapper](self, value: Input, callback: Mapper) Output
    where Mapper: Fun(Input) Output
    {
        callback(value)
    }

    fun applyBorrowed[Input, Output](self, value: Input, callback: &Fun(Input) Output) Output {
        callback(value)
    }
}

fun transform[Context](context: Context, offset: i32) i32 {
    let result = Runner {}.apply(makeSource(), \[context, offset] value -> {
        _ = context;
        Target { value: value.intoTarget().value + offset }
    });
    let borrowed = Runner {}.applyBorrowed(makeSource(), &\[context, offset] value -> {
        _ = context;
        Target { value: value.intoTarget().value + offset }
    });
    result.value + borrowed.value
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if transform(1i32, 2) != 84 { return process::ExitCode(1)!; }
    if transform(true, 3) != 86 { return process::ExitCode(2)!; }
    !()
}
"#,
    )
    .expect("write callable inference source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit callable inference executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run callable inference executable");
    assert_eq!(run.code(), Some(0));
}
