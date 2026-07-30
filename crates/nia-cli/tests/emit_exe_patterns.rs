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
        return (1 as process::ExitCode)!;
    }
    if read_optional(null) != 0 {
        return (2 as process::ExitCode)!;
    }
    if read_error(!7) != 7 {
        return (3 as process::ExitCode)!;
    }
    if read_error(5!) != 5 {
        return (4 as process::ExitCode)!;
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
