// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_process_spawn_raw_and_wait() {
    let root = temp_dir("emit_exe_std_process_spawn_raw_and_wait");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path = b"/bin/true\0";
    var argv: [2]&u8 = [&path.*[0], 0usize as &u8];
    var child = if let !value = process::spawn_raw(&path.*[0], &argv[0], init.raw_envp()) {
        value
    } else error! {
        return process::exit(1)!;
    };
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if not term.exited_success() {
        return process::exit(3)!;
    }
    let code = if let ?value = term.exit_code() {
        value
    } else null {
        return process::exit(4)!;
    };
    if code != 0 {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process spawn");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted std process spawn executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_wait_reports_exit_code() {
    let root = temp_dir("emit_exe_std_process_wait_reports_exit_code");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path = b"/bin/false\0";
    var argv: [2]&u8 = [&path.*[0], 0usize as &u8];
    var child = if let !value = process::spawn_raw(&path.*[0], &argv[0], init.raw_envp()) {
        value
    } else error! {
        return process::exit(1)!;
    };
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if term.exited_success() {
        return process::exit(3)!;
    }
    let code = if let ?value = term.exit_code() {
        value
    } else null {
        return process::exit(4)!;
    };
    if code != 1 {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process false");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted std process false executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_run_reports_term() {
    let root = temp_dir("emit_exe_std_process_command_run_reports_term");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/true\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    let term = if let !value = command.run() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if not term.exited_success() {
        return process::exit(3)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process command run");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted std process command executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_ignore_stdout() {
    let root = temp_dir("emit_exe_std_process_command_can_ignore_stdout");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/echo\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let message = b"ignored-output\0";
    var argv: [3]&u8 = [path.raw_ptr(), &message.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdout(process::StdIo::Ignore).exit().?;
    let term = if let !value = command.run() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if not term.exited_success() {
        return process::exit(3)!;
    }
    var buffer: [64]u8 = [0; 64];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    stdout.write_all(b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process ignore stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process stdio executable");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"ok");
}

#[test]
fn emit_exe_std_process_command_can_ignore_stderr() {
    let root = temp_dir("emit_exe_std_process_command_can_ignore_stderr");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let flag = b"-c\0";
    let script = b"echo ignored-error >&2\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stderr(process::StdIo::Ignore).exit().?;
    let term = if let !value = command.run() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if not term.exited_success() {
        return process::exit(3)!;
    }
    var buffer: [64]u8 = [0; 64];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    stdout.write_all(b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process ignore stderr");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process stderr executable");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"ok");
    assert_eq!(output.stderr, b"");
}

#[test]
fn emit_exe_std_process_command_can_ignore_all_stdio() {
    let root = temp_dir("emit_exe_std_process_command_can_ignore_all_stdio");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let flag = b"-c\0";
    let script = b"cat >/dev/null; echo ignored-output; echo ignored-error >&2\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdin(process::StdIo::Ignore).exit().?;
    command.set_stdout(process::StdIo::Ignore).exit().?;
    command.set_stderr(process::StdIo::Ignore).exit().?;
    let term = if let !value = command.run() {
        value
    } else error! {
        return process::exit(2)!;
    };
    if not term.exited_success() {
        return process::exit(3)!;
    }
    var buffer: [64]u8 = [0; 64];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    stdout.write_all(b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process ignore all stdio");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process all stdio executable");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"ok");
    assert_eq!(output.stderr, b"");
}

#[test]
fn emit_exe_std_process_command_spawn_reports_exec_error() {
    let root = temp_dir("emit_exe_std_process_command_spawn_reports_exec_error");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/definitely/not/a/nia/process-test-binary\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    let child = if let !value = command.spawn() {
        value
    } else error! {
        if error == process::Error::SpawnExec {
            return !{};
        } else {
            return process::exit(2)!;
        }
    };
    _ = child;
    return process::exit(3)!;
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process exec error");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process exec error executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_pipe_stdout() {
    let root = temp_dir("emit_exe_std_process_command_can_pipe_stdout");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let flag = b"-c\0";
    let script = b"printf pipe-output\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdout(process::StdIo::Pipe).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let handle = if let ?value = child.take_stdout() {
        value
    } else null {
        return process::exit(3)!;
    };
    var read_buffer: [16]u8 = [0; 16];
    var reader = io::FileReader::init(init.io(), handle, &mut read_buffer[..]);
    var output: [11]u8 = [0; 11];
    reader.read_exact(&mut output[..]).exit().?;
    handle.close().exit().?;
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(4)!;
    };
    if not term.exited_success() {
        return process::exit(5)!;
    }
    let expected: [11]u8 = [
        b'p',
        b'i',
        b'p',
        b'e',
        b'-',
        b'o',
        b'u',
        b't',
        b'p',
        b'u',
        b't',
    ];
    var index = 0usize;
    while index < output.len() {
        if output[index] != expected[index] {
            return process::exit(6)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process pipe stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process pipe stdout executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_pipe_stdout_reports_eof_after_child_exit() {
    let root = temp_dir("emit_exe_std_process_pipe_stdout_reports_eof_after_child_exit");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/true\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdout(process::StdIo::Pipe).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let handle = if let ?value = child.take_stdout() {
        value
    } else null {
        return process::exit(3)!;
    };
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(4)!;
    };
    if not term.exited_success() {
        return process::exit(5)!;
    }
    var byte: [1]u8 = [0];
    let amount = if let !value = handle.read_some(&mut byte[..]) {
        value
    } else error! {
        return process::exit(6)!;
    };
    handle.close().exit().?;
    if amount != 0usize {
        return process::exit(7)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process pipe stdout eof");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process pipe stdout eof executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_pipe_stdin_and_stdout() {
    let root = temp_dir("emit_exe_std_process_command_can_pipe_stdin_and_stdout");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/cat\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdin(process::StdIo::Pipe).exit().?;
    command.set_stdout(process::StdIo::Pipe).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };

    let stdin_handle = if let ?value = child.take_stdin() {
        value
    } else null {
        return process::exit(3)!;
    };
    var write_buffer: [16]u8 = [0; 16];
    var writer = io::FileWriter::init(init.io(), stdin_handle, &mut write_buffer[..]);
    writer.write_all(b"roundtrip").exit().?;
    writer.flush().exit().?;
    stdin_handle.close().exit().?;

    let stdout_handle = if let ?value = child.take_stdout() {
        value
    } else null {
        return process::exit(4)!;
    };
    var read_buffer: [16]u8 = [0; 16];
    var reader = io::FileReader::init(init.io(), stdout_handle, &mut read_buffer[..]);
    var output: [9]u8 = [0; 9];
    reader.read_exact(&mut output[..]).exit().?;
    stdout_handle.close().exit().?;

    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(5)!;
    };
    if not term.exited_success() {
        return process::exit(6)!;
    }

    let expected: [9]u8 = [b'r', b'o', b'u', b'n', b'd', b't', b'r', b'i', b'p'];
    var index = 0usize;
    while index < output.len() {
        if output[index] != expected[index] {
            return process::exit(7)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process pipe stdin stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process pipe stdin stdout executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_wait_closes_owned_stdin_pipe() {
    let root = temp_dir("emit_exe_std_process_wait_closes_owned_stdin_pipe");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/cat\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdin(process::StdIo::Pipe).exit().?;
    command.set_stdout(process::StdIo::Ignore).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(3)!;
    };
    if not term.exited_success() {
        return process::exit(4)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process wait closes stdin");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process wait closes stdin executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_wait_is_repeatable() {
    let root = temp_dir("emit_exe_std_process_wait_is_repeatable");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/true\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let first = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(3)!;
    };
    let second = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(4)!;
    };
    if not first.exited_success() or not second.exited_success() {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process repeat wait");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process repeat wait executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_try_wait_reports_exit() {
    let root = temp_dir("emit_exe_std_process_try_wait_reports_exit");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/true\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    var spins = 0usize;
    while spins < 100000usize {
        let maybe = if let !value = child.try_wait() {
            value
        } else error! {
            return process::exit(3)!;
        };
        if let ?term = maybe {
            if not term.exited_success() {
                return process::exit(4)!;
            }
            let again = if let !value = child.try_wait() {
                value
            } else error! {
                return process::exit(5)!;
            };
            if let ?cached = again {
                if not cached.exited_success() {
                    return process::exit(6)!;
                }
            } else null {
                return process::exit(7)!;
            }
            return !{};
        } else null {}
        spins += 1usize;
    }
    return process::exit(8)!;
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process try wait");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process try wait executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_try_wait_keeps_owned_stdin_pipe_open() {
    let root = temp_dir("emit_exe_std_process_try_wait_keeps_owned_stdin_pipe_open");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/cat\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdin(process::StdIo::Pipe).exit().?;
    command.set_stdout(process::StdIo::Ignore).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let first = if let !value = child.try_wait() {
        value
    } else error! {
        return process::exit(3)!;
    };
    if let ?term = first {
        _ = term;
        return process::exit(4)!;
    } else null {}
    child.close_stdin().exit().?;
    let term = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(5)!;
    };
    if not term.exited_success() {
        return process::exit(6)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process try wait keeps stdin");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout("run emitted std process try wait keeps stdin executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_kill_terminates_child() {
    let root = temp_dir("emit_exe_std_process_kill_terminates_child");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let flag = b"-c\0";
    let script = b"while true; do sleep 1; done\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdout(process::StdIo::Ignore).exit().?;
    command.set_stderr(process::StdIo::Ignore).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let term = if let !value = child.kill() {
        value
    } else error! {
        return process::exit(3)!;
    };
    let signal = if let ?value = term.signal_code() {
        value
    } else null {
        return process::exit(4)!;
    };
    if signal != 15 {
        return process::exit(5)!;
    }
    let cached = if let !value = child.wait() {
        value
    } else error! {
        return process::exit(6)!;
    };
    let cached_signal = if let ?value = cached.signal_code() {
        value
    } else null {
        return process::exit(7)!;
    };
    if cached_signal != 15 {
        return process::exit(8)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process kill");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process kill executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_kill_with_uses_requested_signal() {
    let root = temp_dir("emit_exe_std_process_kill_with_uses_requested_signal");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let flag = b"-c\0";
    let script = b"while true; do sleep 1; done\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_stdout(process::StdIo::Ignore).exit().?;
    command.set_stderr(process::StdIo::Ignore).exit().?;
    var child = if let !value = command.spawn() {
        value
    } else error! {
        return process::exit(2)!;
    };
    let term = if let !value = child.kill_with(process::Signal::Kill) {
        value
    } else error! {
        return process::exit(3)!;
    };
    let signal = if let ?value = term.signal_code() {
        value
    } else null {
        return process::exit(4)!;
    };
    if signal != 9 {
        return process::exit(5)!;
    }
    let cached = if let !value = child.try_wait() {
        value
    } else error! {
        return process::exit(6)!;
    };
    if let ?cached_term = cached {
        let cached_signal = if let ?value = cached_term.signal_code() {
            value
        } else null {
            return process::exit(7)!;
        };
        if cached_signal != 9 {
            return process::exit(8)!;
        }
    } else null {
        return process::exit(9)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process kill with signal");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output =
        Command::new(&exe).output_timeout("run emitted std process kill with signal executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_set_cwd() {
    let root = temp_dir("emit_exe_std_process_command_can_set_cwd");
    let child_dir = root.join("child-cwd");
    std::fs::create_dir(&child_dir).expect("create child cwd");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let cwd_literal = format!("{}\0", child_dir.display());
    std::fs::write(
        &main,
        format!(
            r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {{
    let path_bytes = b"/bin/sh\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {{
        value
    }} else null {{
        return process::exit(1)!;
    }};
    let cwd_bytes = b"{cwd_literal}";
    let cwd = if let ?value = std::CStr::from_bytes(cwd_bytes) {{
        value
    }} else null {{
        return process::exit(2)!;
    }};
    let flag = b"-c\0";
    let script = b"test \"$(basename \"$PWD\")\" = child-cwd\0";
    var argv: [4]&u8 = [path.raw_ptr(), &flag.*[0], &script.*[0], 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_cwd(cwd);
    let term = if let !value = command.run() {{
        value
    }} else error! {{
        return process::exit(3)!;
    }};
    if not term.exited_success() {{
        return process::exit(4)!;
    }}
    !{{}}
}}
"#,
            cwd_literal = cwd_literal
        ),
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process cwd");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process cwd executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_reports_cwd_spawn_stage() {
    let root = temp_dir("emit_exe_std_process_command_reports_cwd_spawn_stage");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let path_bytes = b"/bin/true\0";
    let path = if let ?value = std::CStr::from_bytes(path_bytes) {
        value
    } else null {
        return process::exit(1)!;
    };
    let cwd_bytes = b"/definitely/not/a/nia/process-test-cwd\0";
    let cwd = if let ?value = std::CStr::from_bytes(cwd_bytes) {
        value
    } else null {
        return process::exit(2)!;
    };
    var argv: [2]&u8 = [path.raw_ptr(), 0usize as &u8];
    var command = process::Command::init(path, &argv[0], init.raw_envp());
    command.set_cwd(cwd);
    let child = if let !value = command.spawn() {
        value
    } else error! {
        if error == process::Error::SpawnCwd {
            return !{};
        } else {
            return process::exit(3)!;
        }
    };
    _ = child;
    return process::exit(4)!;
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std process cwd stage");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout("run emitted std process cwd stage executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_discards_indirect_return_call_in_loop() {
    let root = temp_dir("emit_exe_discards_indirect_return_call_in_loop");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process::{Init, ExitCode};

fn should_skip(arg: std::process::Arg) bool {
    _ = arg;
    false
}

pub fn main(init: Init) ExitCode!void {
    let args = init.args();
    var paths = args.skip_program();
    while true {
        let path = if let ?value = paths.next() {
            value
        } else null {
            break;
        };
        if should_skip(path) {
            _ = paths.next();
            continue;
        }
        _ = path;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe discarded indirect return call");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_resolves_unqualified_extension_helpers() {
    let root = temp_dir("emit_exe_resolves_unqualified_extension_helpers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct S {}

extend S {
    fn helper() i32 {
        41
    }

    fn method(&self) i32 {
        helper() + 1
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let value = S {};
    if value.method() != 42 {
        return process::exit(1)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe extension helper");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted extension helper executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_cstr_from_std_root() {
    let root = temp_dir("emit_exe_exposes_cstr_from_std_root");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let value = if let ?value = std::CStr::from_bytes(b"nia\0") {
        value
    } else null {
        return process::exit(3)!;
    };
    if value.len() != 3usize {
        return process::exit(1)!;
    }
    let bytes = value.bytes();
    if bytes[0] != b'n' or bytes[1] != b'i' or bytes[2] != b'a' {
        return process::exit(2)!;
    }
    if let ?invalid = std::CStr::from_bytes(b"nia") {
        _ = invalid;
        return process::exit(4)!;
    } else null {}
    let ptr = b"nia\0".get_ptr_read();
    if ptr[0] != b'n' or ptr[1] != b'i' or ptr[2] != b'a' or ptr[3] != 0u8 {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe std root CStr");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted std root CStr executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_links_freestanding_executable() {
    let root = temp_dir("emit_exe_links_freestanding_executable");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    (7 as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_runs_slice_trait_object_dynamic_dispatch() {
    let root = temp_dir("emit_exe_runs_slice_trait_object_dynamic_dispatch");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

trait Source {
    fn get(& self) i32;
}

extend[T] [T] : Source {
    fn get(& self) i32 {
        self.len() as i32
    }
}

fn read(source: & Source) i32 {
    source.get()
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var values: [3]i32 = [1, 2, 3];
    (read(&values[..]) as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(3));
}

#[test]
fn emit_exe_links_slice_trait_object_adapters_from_multiple_modules() {
    let root = temp_dir("emit_exe_links_slice_trait_object_adapters_from_multiple_modules");
    let main = root.join("main.nia");
    let left = root.join("left.nia");
    let right = root.join("right.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
pub module left;
pub module right;

using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    left::write(init).?;
    right::write(init).?;
    !{}
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        &left,
        r#"
using std::io;
using std::process;

pub fn write(init: process::Init) process::ExitCode!void {
    var buffer = [_]u8[0; 128];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer);
    defer stdout.flush().exit().?;
    let text = b"left";
    stdout.print("{}\n", &[&text.*[..]]).exit()
}
"#,
    )
    .expect("write left source");
    std::fs::write(
        &right,
        r#"
using std::io;
using std::process;

pub fn write(init: process::Init) process::ExitCode!void {
    var buffer = [_]u8[0; 128];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer);
    defer stdout.flush().exit().?;
    let text = b"right";
    stdout.print("{}\n", &[&text.*[..]]).exit()
}
"#,
    )
    .expect("write right source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_runs_slice_trait_object_dispatch_with_zst_argument() {
    let root = temp_dir("emit_exe_runs_slice_trait_object_dispatch_with_zst_argument");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Empty {}

trait Source {
    fn add(& self, empty: Empty, rhs: i32) i32;
}

extend[T] [T] : Source {
    fn add(& self, empty: Empty, rhs: i32) i32 {
        _ = empty;
        self.len() as i32 + rhs
    }
}

fn read(source: & Source) i32 {
    source.add({}, 4)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var values: [3]i32 = [1, 2, 3];
    (read(&values[..]) as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_links_freestanding_u128_division_builtins() {
    let root = temp_dir("emit_exe_links_freestanding_u128_division_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let argc = init.argc();
    let value = (1u128 << 100u32) + 12345u128;
    let by = argc as u128 + 53u128;
    let q = value / by;
    let r = value % by;
    if q * by + r != value {
        return (1 as process::ExitCode)!;
    }
    if r >= by {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_links_freestanding_i128_division_builtins() {
    let root = temp_dir("emit_exe_links_freestanding_i128_division_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let argc = init.argc();
    let base = (1i128 << 100u32) + 12345i128;
    let divisor = argc as i128 + 53i128;

    let q0 = base / divisor;
    let r0 = base % divisor;
    if q0 * divisor + r0 != base {
        return (1 as process::ExitCode)!;
    }
    if r0 < 0i128 or r0 >= divisor {
        return (2 as process::ExitCode)!;
    }

    let neg_base = -base;
    let q1 = neg_base / divisor;
    let r1 = neg_base % divisor;
    if q1 * divisor + r1 != neg_base {
        return (3 as process::ExitCode)!;
    }
    if r1 > 0i128 or r1 <= -divisor {
        return (4 as process::ExitCode)!;
    }

    let neg_divisor = -divisor;
    let q2 = base / neg_divisor;
    let r2 = base % neg_divisor;
    if q2 * neg_divisor + r2 != base {
        return (5 as process::ExitCode)!;
    }
    if r2 < 0i128 or r2 >= divisor {
        return (6 as process::ExitCode)!;
    }

    let q3 = neg_base / neg_divisor;
    let r3 = neg_base % neg_divisor;
    if q3 * neg_divisor + r3 != neg_base {
        return (7 as process::ExitCode)!;
    }
    if r3 > 0i128 or r3 <= -divisor {
        return (8 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exit_code_is_open_enum() {
    let root = temp_dir("emit_exe_exit_code_is_open_enum");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::fs;

using process::{ExitCode, exit};

fn pick(flag: bool) ExitCode {
    if flag {
        11 as ExitCode
    } else {
        ExitCode::Success
    }
}

fn pick_result() fs::Error!ExitCode {
    !pick(true)
}

fn fail_with_no_space() fs::Error!void {
    fs::Error::NoSpace!
}

pub fn main(init: process::Init) ExitCode!void {
    _ = init;

    if (ExitCode::Success as i32) != 0 {
        return exit(1)!;
    }
    if (exit(11) as i32) != 11 {
        return exit(2)!;
    }
    if (fs::Error::NotFound.as_exit_code() as i32) != 2 {
        return exit(3)!;
    }
    let picked = pick_result().exit().?;
    if (picked as i32) != 11 {
        return exit(4)!;
    }
    fail_with_no_space().exit()
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(28));
}

#[test]
fn emit_exe_can_use_direct_std_modules() {
    let root = temp_dir("emit_exe_can_use_direct_std_modules");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var writer = io::DiscardingWriter::init();
    if let !ok = writer.write_all(b"nia") { _ = ok; } else error! { return (1 as process::ExitCode)!; }
    if writer.len() != 3 {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_math_usize_helpers() {
    let root = temp_dir("emit_exe_can_use_std_math_usize_helpers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::math;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if 0usize.is_power_of_two() {
        return (1 as process::ExitCode)!;
    }
    if not 4096usize.is_power_of_two() {
        return (2 as process::ExitCode)!;
    }
    if let ?value = 10usize.checked_add(5usize) { if value != 15usize {
                return (3 as process::ExitCode)!;
            } } else null { return (4 as process::ExitCode)!; }
    if let ?value = 18446744073709551615usize.checked_add(1usize) { _ = value;
            return (5 as process::ExitCode)!; } else null { }
    if let ?value = 12usize.checked_mul(3usize) { if value != 36usize {
                return (6 as process::ExitCode)!;
            } } else null { return (7 as process::ExitCode)!; }
    if let ?value = 4611686018427387904usize.checked_mul(4usize) { _ = value;
            return (8 as process::ExitCode)!; } else null { }
    if let ?value = 17usize.align_forward(8usize) { if value != 24usize {
                return (9 as process::ExitCode)!;
            } } else null { return (10 as process::ExitCode)!; }
    if let ?value = 17usize.align_forward(3usize) { _ = value;
            return (11 as process::ExitCode)!; } else null { }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_math_checked_integer_helpers() {
    let root = temp_dir("emit_exe_can_use_std_math_checked_integer_helpers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::math;
using std::process;

fn add_checked_same[T](lhs: T, rhs: T) ?T
where T: math::CheckedAdd[T, Output = T]
{
    lhs.checked_add(rhs)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    if let ?value = add_checked_same[u8](250u8, 5u8) { if value != 255u8 { return process::exit(1)!; } } else null { return process::exit(2)!; }
    if let ?value = 255u8.checked_add(1u8) { _ = value; return process::exit(3)!; } else null { }
    if let ?value = 10u16.checked_sub(3u16) { if value != 7u16 { return process::exit(4)!; } } else null { return process::exit(5)!; }
    if let ?value = 0u16.checked_sub(1u16) { _ = value; return process::exit(6)!; } else null { }
    if let ?value = 70000u32.checked_mul(60000u32) { if value != 4200000000u32 { return process::exit(7)!; } } else null { return process::exit(8)!; }
    if let ?value = 0xffffffffu32.checked_mul(2u32) { _ = value; return process::exit(9)!; } else null { }
    if let ?value = 100u64.checked_div(4u64) { if value != 25u64 { return process::exit(10)!; } } else null { return process::exit(11)!; }
    if let ?value = 100u64.checked_div(0u64) { _ = value; return process::exit(12)!; } else null { }
    if let ?value = 100u128.checked_rem(7u128) { if value != 2u128 { return process::exit(13)!; } } else null { return process::exit(14)!; }
    if let ?value = 100u128.checked_rem(0u128) { _ = value; return process::exit(15)!; } else null { }
    if let ?value = 9usize.checked_sub(4usize) { if value != 5usize { return process::exit(16)!; } } else null { return process::exit(17)!; }

    if let ?value = (-5i8).checked_neg() { if value != 5i8 { return process::exit(18)!; } } else null { return process::exit(19)!; }
    if let ?value = i8::MIN.checked_neg() { _ = value; return process::exit(20)!; } else null { }
    if let ?value = (-123i16).checked_abs() { if value != 123i16 { return process::exit(21)!; } } else null { return process::exit(22)!; }
    if let ?value = i16::MIN.checked_abs() { _ = value; return process::exit(23)!; } else null { }
    if let ?value = i32::MAX.checked_add(1i32) { _ = value; return process::exit(24)!; } else null { }
    if let ?value = (-10i32).checked_add(5i32) { if value != -5i32 { return process::exit(25)!; } } else null { return process::exit(26)!; }
    if let ?value = i64::MIN.checked_sub(1i64) { _ = value; return process::exit(27)!; } else null { }
    if let ?value = 10i64.checked_sub(-5i64) { if value != 15i64 { return process::exit(28)!; } } else null { return process::exit(29)!; }
    if let ?value = i128::MIN.checked_mul(-1i128) { _ = value; return process::exit(30)!; } else null { }
    if let ?value = 12i128.checked_mul(-3i128) { if value != -36i128 { return process::exit(31)!; } } else null { return process::exit(32)!; }
    if let ?value = isize::MIN.checked_div(-1isize) { _ = value; return process::exit(33)!; } else null { }
    if let ?value = (-9isize).checked_div(3isize) { if value != -3isize { return process::exit(34)!; } } else null { return process::exit(35)!; }
    if let ?value = (-9isize).checked_rem(0isize) { _ = value; return process::exit(36)!; } else null { }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_process_args_without_raw_argv() {
    let root = temp_dir("emit_exe_exposes_process_args_without_raw_argv");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    var args = init.args();
    if args.len() != 3 {
        return (1 as process::ExitCode)!;
    }
    if let ?program = args.program() { if program.is_empty() {
            return (9 as process::ExitCode)!;
        } } else null { return (10 as process::ExitCode)!; }
    var iter = args.skip_program();
    if iter.remaining() != 2usize {
        return (11 as process::ExitCode)!;
    }
    var first_arg = if let ?value = iter.next() { value } else null { return (2 as process::ExitCode)!; };
    if iter.remaining() != 1usize {
        return (12 as process::ExitCode)!;
    }
    var second_arg = if let ?value = iter.next() { value } else null { return (3 as process::ExitCode)!; };
    if iter.remaining() != 0usize {
        return (13 as process::ExitCode)!;
    }
    var for_count = 0usize;
    for arg in args.skip_program() {
        if for_count == 0usize {
            if arg.len() != 3usize {
                return (18 as process::ExitCode)!;
            }
        } else if for_count == 1usize {
            if let !value = fmt::parse[u16](arg) {
                if value != 1234u16 {
                    return (19 as process::ExitCode)!;
                }
            } else error! {
                return (20 as process::ExitCode)!;
            }
        } else {
            return (21 as process::ExitCode)!;
        }
        for_count += 1usize;
    }
    if for_count != 2usize {
        return (22 as process::ExitCode)!;
    }
    var first = first_arg.bytes();
    var second = second_arg.bytes();
    if first.len() != 3 {
        return (4 as process::ExitCode)!;
    }
    if first[0] != 110u8 or first[1] != 105u8 or first[2] != 97u8 {
        return (5 as process::ExitCode)!;
    }
    if second.len() != 4 {
        return (6 as process::ExitCode)!;
    }
    if let !value = fmt::parse[u16](second_arg) { if value != 1234u16 {
            return (14 as process::ExitCode)!;
        } } else error! { return (15 as process::ExitCode)!; }
    if let !value = fmt::parse_radix[u16](second_arg, 16u32) { if value != 0x1234u16 {
            return (16 as process::ExitCode)!;
        } } else error! { return (17 as process::ExitCode)!; }
    var storage: [16]u8 = [0; 16];
    var writer = io::FixedBufferWriter::init(&mut storage[..]);
    writer.print("{:_>5.2}", &[&first_arg]).exit().?;
    let written = writer.written();
    if written.len() != 5usize or written[0] != b'_' or written[1] != b'_' or written[2] != b'_' or written[3] != b'n' or written[4] != b'i' {
        return (8 as process::ExitCode)!;
    }
    if let ?value = iter.next() { _ = value;
            return (7 as process::ExitCode)!; } else null { }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .arg("nia")
        .arg("1234")
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_process_env_as_values() {
    let root = temp_dir("emit_exe_exposes_process_env_as_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fn starts_with_needle(bytes: &[u8]) bool {
    let needle: &[u8] = b"NIA_TEST_ENV=ok";
    if bytes.len() < needle.len() {
        return false;
    }
    var index = 0usize;
    while index < needle.len() {
        if bytes[index] != needle[index] {
            return false;
        }
        index += 1usize;
    }
    true
}

pub fn main(init: process::Init) process::ExitCode!void {
    for item in init.env().iter() {
        if starts_with_needle(item.bytes()) {
            return !{};
        }
    }
    return (2 as process::ExitCode)!;
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .env("NIA_TEST_ENV", "ok")
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_error_union_conversion_extension() {
    let root = temp_dir("emit_exe_can_use_error_union_conversion_extension");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

enum ParseError: i32 {
    Bad = 1,
    _
}

enum AppError: i32 {
    InvalidInput = 7,
    _
}

fn map_parse_error(error: ParseError) AppError {
    _ = error;
    AppError::InvalidInput
}

fn parse() ParseError!i32 {
    ParseError::Bad!
}

extend[T] ParseError!T {
    fn as_app_error(self) AppError!T {
        if let !value = self { !value } else err! { map_parse_error(err)! }
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if let !value = parse().as_app_error() { return (value as process::ExitCode)!; } else err! { return (err as i32 as process::ExitCode)!; }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_local_pointer_binding_patterns_destructure_values() {
    let root = temp_dir("emit_exe_local_pointer_binding_patterns_destructure_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var left = 20;
    var right = 22;

    let &x = &left;
    var &mut y: i32 = &mut right;
    y += 1;

    if x + y != 43 {
        return (1 as process::ExitCode)!;
    }
    if right != 22 {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_if_pattern_matches_nested_error_optional_once() {
    let root = temp_dir("emit_exe_if_pattern_matches_nested_error_optional_once");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

var calls: i32 = 0;

fn next(flag: bool) ?(i32!i32) {
    calls += 1;
    if flag {
        let ok: i32!i32 = !7;
        ?ok
    } else {
        let err: i32!i32 = 5!;
        ?err
    }
}

fn classify(value: ?(i32!i32)) i32 {
    if let ?!ok = value {
        ok
    } else ?err! {
        err
    } else null {
        0
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var total = 0;
    if let ?!value = next(true) {
        total = value;
    } else ?err! {
        total = err + 10;
    } else null {
        total = 20;
    }
    if calls != 1 {
        return (1 as process::ExitCode)!;
    }
    if total != 7 {
        return (2 as process::ExitCode)!;
    }
    if classify(next(false)) != 5 {
        return (3 as process::ExitCode)!;
    }
    if calls != 2 {
        return (4 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_mut_ref_receiver_updates_original_aggregate() {
    let root = temp_dir("emit_exe_mut_ref_receiver_updates_original_aggregate");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Counter {
    value: i32,
}

extend Counter {
    fn init() Counter {
        { value: 0 }
    }

    fn add(&mut self, amount: i32) void {
        self.value += amount;
    }

    fn get(&self) i32 {
        self.value
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var counter = Counter::init();
    counter.add(7);
    if counter.get() != 7 {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
