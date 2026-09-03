// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_optional_queries_and_transforms() {
    let root = temp_dir("emit_exe_std_optional_queries_and_transforms");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::optional;
using std::process;

fn double(value: ?i32) ?i32 {
    value.map(&\value: i32 -> value * 2)
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let present = double(?21);
    if not present.isPresent() {
        return process::exit(1)!;
    }
    if present is ?value {
        if value != 42 { return process::exit(2)!; }
    }
    if double(null).isNull() == false or double(null).map(&\value: i32 -> value).isPresent() {
        return process::exit(3)!;
    }
    if double(?1).andThen(&\value: i32 -> if value == 2 { ?41 } else { null }) is ?value {
        if value != 41 { return process::exit(4)!; }
    } else {
        return process::exit(5)!;
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
