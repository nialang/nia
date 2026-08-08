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
    let mut argv: [2]&u8 = [&path[0], 0usize as &u8];
    let mut child = switch process::spawnRaw(&path[0], &argv[0], init.rawEnvp()) {
        !value => {
            value
        },
        error! => {
            return process::exit(1)!;
        },
    };
    let pid: process::ProcessId = child.pid();
    if pid.raw() <= 0 {
        return process::exit(3)!;
    }
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    if not term.succeeded() {
        return process::exit(3)!;
    }
    let code = switch term.exitCode() {
        ?value => {
            value
        },
        null => {
            return process::exit(4)!;
        },
    };
    if code != 0 {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process spawn");
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
    let mut argv: [2]&u8 = [&path[0], 0usize as &u8];
    let mut child = switch process::spawnRaw(&path[0], &argv[0], init.rawEnvp()) {
        !value => {
            value
        },
        error! => {
            return process::exit(1)!;
        },
    };
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    if term.succeeded() {
        return process::exit(3)!;
    }
    let code = switch term.exitCode() {
        ?value => {
            value
        },
        null => {
            return process::exit(4)!;
        },
    };
    if code != 1 {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process false");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted std process false executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_encodes_text_arguments() {
    let root = temp_dir("emit_exe_std_process_command_encodes_text_arguments");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let invalidPath = process::Command::init(std::PathView::init(&"bad\0path"), init.env());
    switch invalidPath.spawn() {
        !child => {
            _ = child;
            return process::exit(7)!;
        },
        process::Error::Path(std::fs::PathError::ContainsNul)! => {},
        error! => {
            _ = error;
            return process::exit(8)!;
        },
    }

    let invalidArguments: [2]&[char] = [&"valid", &"bad\0argument"];
    let invalid = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withArguments(&invalidArguments);
    switch invalid.spawn() {
        !child => {
            _ = child;
            return process::exit(1)!;
        },
        process::Error::ArgumentContainsNul(1)! => {},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }

    let mut tinyStorage: [1]u8 = [0];
    let mut fixed = mem::FixedBufferAllocator::init(&mut tinyStorage[..]);
    let allocator: &mut mem::Allocator = &mut fixed;
    let noMemory = process::Command::init(std::PathView::init(&"/bin/true"), init.env());
    switch noMemory.spawnWithAllocator(allocator) {
        !child => {
            _ = child;
            return process::exit(5)!;
        },
        process::Error::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(6)!;
        },
    }

    let arguments: [4]&[char] = [&"-c", &"test \"$1\" = \"λ\"", &"nia", &"λ"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments);
    let term = switch command.run() {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    if not term.succeeded() {
        return process::exit(4)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process typed command arguments");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status =
        Command::new(&exe).status_timeout("run emitted std process typed command executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_configures_exact_environment() {
    let root = temp_dir("emit_exe_std_process_command_configures_exact_environment");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

fn expectEnvironmentError(
    init: process::Init,
    entries: &[process::EnvEntry],
    expectedIndex: usize,
    expectedCause: process::EnvEntryError,
) process::ExitCode!void {
    let command = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withEnvironment(entries);
    switch command.spawn() {
        !child => {
            _ = child;
            return process::exit(20)!;
        },
        process::Error::Environment { index, cause }! => {
            if index != expectedIndex {
                return process::exit(21)!;
            }
            switch cause {
                process::EnvEntryError::EmptyName => switch expectedCause {
                    process::EnvEntryError::EmptyName => {},
                    _ => return process::exit(22)!,
                },
                process::EnvEntryError::NameContainsEquals => switch expectedCause {
                    process::EnvEntryError::NameContainsEquals => {},
                    _ => return process::exit(23)!,
                },
                process::EnvEntryError::NameContainsNul => switch expectedCause {
                    process::EnvEntryError::NameContainsNul => {},
                    _ => return process::exit(24)!,
                },
                process::EnvEntryError::ValueContainsNul => switch expectedCause {
                    process::EnvEntryError::ValueContainsNul => {},
                    _ => return process::exit(25)!,
                },
                process::EnvEntryError::DuplicateName(firstIndex) => switch expectedCause {
                    process::EnvEntryError::DuplicateName(expectedFirstIndex) => {
                        if firstIndex != expectedFirstIndex {
                            return process::exit(26)!;
                        }
                    },
                    _ => return process::exit(27)!,
                },
            }
        },
        error! => {
            _ = error;
            return process::exit(28)!;
        },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    let exactEnvironment: [1]process::EnvEntry = [process::EnvEntry::init(&"MODE", &"λ")];
    let exactArguments: [2]&[char] = [
        &"-c",
        &"test \"$MODE\" = \"λ\" && test -z \"${NIA_INHERITED_SENTINEL+x}\"",
    ];
    let exact = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&exactArguments)
        .withEnvironment(&exactEnvironment);
    let exactTerm = exact.run().exit().?;
    if not exactTerm.succeeded() {
        return process::exit(1)!;
    }

    let emptyArguments: [2]&[char] = [
        &"-c",
        &"test -z \"${NIA_INHERITED_SENTINEL+x}\" && test -z \"${MODE+x}\"",
    ];
    let empty = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&emptyArguments)
        .withoutEnvironment();
    let emptyTerm = empty.run().exit().?;
    if not emptyTerm.succeeded() {
        return process::exit(2)!;
    }

    let emptyName: [1]process::EnvEntry = [process::EnvEntry::init(&"", &"value")];
    expectEnvironmentError(init, &emptyName, 0, process::EnvEntryError::EmptyName).?;

    let equalsName: [1]process::EnvEntry = [process::EnvEntry::init(&"BAD=NAME", &"value")];
    expectEnvironmentError(init, &equalsName, 0, process::EnvEntryError::NameContainsEquals).?;

    let nulName: [1]process::EnvEntry = [process::EnvEntry::init(&"BAD\0NAME", &"value")];
    expectEnvironmentError(init, &nulName, 0, process::EnvEntryError::NameContainsNul).?;

    let nulValue: [1]process::EnvEntry = [process::EnvEntry::init(&"NAME", &"bad\0value")];
    expectEnvironmentError(init, &nulValue, 0, process::EnvEntryError::ValueContainsNul).?;

    let duplicate: [2]process::EnvEntry = [
        process::EnvEntry::init(&"NAME", &"first"),
        process::EnvEntry::init(&"NAME", &"second"),
    ];
    expectEnvironmentError(init, &duplicate, 1, process::EnvEntryError::DuplicateName(0)).?;

    let largeValue: [512]char = [_]char['x'; 512];
    let largeEnvironment: [1]process::EnvEntry = [process::EnvEntry::init(&"LARGE", &largeValue)];
    let mut storage: [128]u8 = [_]u8[0; 128];
    let mut fixed = mem::FixedBufferAllocator::init(&mut storage[..]);
    let allocator: &mut mem::Allocator = &mut fixed;
    let noMemory = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withEnvironment(&largeEnvironment);
    switch noMemory.spawnWithAllocator(allocator) {
        !child => {
            _ = child;
            return process::exit(3)!;
        },
        process::Error::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }
    !{}
}
"#,
    )
    .expect("write exact command environment source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process exact environment");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new(&exe)
        .env("NIA_INHERITED_SENTINEL", "present")
        .status_timeout("run emitted std process environment executable");
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
    let arguments: [1]&[char] = [&"ignored-output"];
    let command = process::Command::init(std::PathView::init(&"/bin/echo"), init.env())
        .withArguments(&arguments)
        .withStdout(process::StdIo::Ignore);
    let term = switch command.run() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    if not term.succeeded() {
        return process::exit(3)!;
    }
    let mut buffer: [64]u8 = [0; 64];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    stdout.writeAll(&b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process ignore stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process stdio executable");
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
    let arguments: [2]&[char] = [&"-c", &"echo ignored-error >&2"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStderr(process::StdIo::Ignore);
    let term = switch command.run() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    if not term.succeeded() {
        return process::exit(3)!;
    }
    let mut buffer: [64]u8 = [0; 64];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    stdout.writeAll(&b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process ignore stderr");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process stderr executable");
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
    let arguments: [2]&[char] = [&"-c", &"cat >/dev/null; echo ignored-output; echo ignored-error >&2"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStdin(process::StdIo::Ignore)
        .withStdout(process::StdIo::Ignore)
        .withStderr(process::StdIo::Ignore);
    let term = switch command.run() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    if not term.succeeded() {
        return process::exit(3)!;
    }
    let mut buffer: [64]u8 = [0; 64];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    stdout.writeAll(&b"ok").exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process ignore all stdio");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process all stdio executable");
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
    let command = process::Command::init(
        std::PathView::init(&"/definitely/not/a/nia/process-test-binary"),
        init.env(),
    );
    let child = switch command.spawn() {
        !value => {
            value
        },
        process::Error::Spawn(process::SpawnError::Exec(process::SystemError::NotFound))! => return !{},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    };
    _ = child;
    return process::exit(3)!;
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process exec error");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process exec error executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_failed_spawn_cleans_pipe_handles() {
    let root = temp_dir("emit_exe_std_process_failed_spawn_cleans_pipe_handles");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let badPath = std::PathView::init(&"/definitely/not/a/nia/process-test-binary");
    let mut index = 0;
    while index < 128 {
        let bad = process::Command::init(badPath, init.env())
            .withStdin(process::StdIo::Pipe)
            .withStdout(process::StdIo::Pipe)
            .withStderr(process::StdIo::Pipe);
        let child = switch bad.spawn() {
            !value => {
                value
            },
            error! => {
                if error is process::Error::Spawn(process::SpawnError::Exec(_)) {
                    index += 1;
                    continue;
                } else {
                    return process::exit(2)!;
                }
            },
        };
        _ = child;
        return process::exit(3)!;
    }

    let good = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withStdin(process::StdIo::Ignore)
        .withStdout(process::StdIo::Ignore)
        .withStderr(process::StdIo::Ignore);
    let term = switch good.run() {
        !value => {
            value
        },
        error! => {
            return process::exit(5)!;
        },
    };
    if not term.succeeded() {
        return process::exit(6)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process failed spawn cleanup");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout_without_resources(
        "run emitted std process failed spawn cleanup executable",
    );
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
    let arguments: [2]&[char] = [&"-c", &"printf pipe-output"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStdout(process::StdIo::Pipe);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let mut stdout = switch child.takeStdout() {
        ?value => {
            value
        },
        null => {
            return process::exit(3)!;
        },
    };
    let mut readBuffer: [16]u8 = [0; 16];
    let mut output: [11]u8 = [0; 11];
    {
        let mut reader = stdout.buffered(&mut readBuffer[..]);
        reader.readExact(&mut output[..]).exit().?;
    }
    stdout.close().exit().?;
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(4)!;
        },
    };
    if not term.succeeded() {
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
    let mut index = 0usize;
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

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process pipe stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process pipe stdout executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_pipe_stderr() {
    let root = temp_dir("emit_exe_std_process_command_can_pipe_stderr");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let arguments: [2]&[char] = [&"-c", &"printf pipe-error >&2"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStderr(process::StdIo::Pipe);
    let mut child = switch command.spawn() {
        !value => value,
        error! => {
            _ = error;
            return process::exit(1)!;
        },
    };
    let mut stderr = switch child.takeStderr() {
        ?value => value,
        null => return process::exit(2)!,
    };
    let mut output: [10]u8 = [0; 10];
    stderr.readExact(&mut output[..]).exit().?;
    stderr.close().exit().?;
    let term = child.wait().exit().?;
    if not term.succeeded() {
        return process::exit(3)!;
    }
    let expected: [10]u8 = [b'p', b'i', b'p', b'e', b'-', b'e', b'r', b'r', b'o', b'r'];
    if not (&output[..]).equals(&expected[..]) {
        return process::exit(4)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process pipe stderr");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process pipe stderr executable");
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
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let command = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withStdout(process::StdIo::Pipe);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let mut stdout = switch child.takeStdout() {
        ?value => {
            value
        },
        null => {
            return process::exit(3)!;
        },
    };
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(4)!;
        },
    };
    if not term.succeeded() {
        return process::exit(5)!;
    }
    let mut byte: [1]u8 = [0];
    let amount = switch stdout.read(&mut byte[..]) {
        !value => {
            value
        },
        error! => {
            return process::exit(6)!;
        },
    };
    stdout.close().exit().?;
    stdout.close().exit().?;
    switch stdout.read(&mut byte[..]) {
        !value => {
            _ = value;
            return process::exit(8)!;
        },
        io::Error::Closed! => {},
        error! => {
            _ = error;
            return process::exit(9)!;
        },
    }
    if amount != 0usize {
        return process::exit(7)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process pipe stdout eof");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process pipe stdout eof executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/cat"), init.env())
        .withStdin(process::StdIo::Pipe)
        .withStdout(process::StdIo::Pipe);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };

    let mut stdin = switch child.takeStdin() {
        ?value => {
            value
        },
        null => {
            return process::exit(3)!;
        },
    };
    let mut writeBuffer: [16]u8 = [0; 16];
    {
        let mut writer = stdin.buffered(&mut writeBuffer[..]);
        writer.writeAll(&b"roundtrip").exit().?;
        writer.flush().exit().?;
    }
    stdin.close().exit().?;

    let mut stdout = switch child.takeStdout() {
        ?value => {
            value
        },
        null => {
            return process::exit(4)!;
        },
    };
    let mut output: [9]u8 = [0; 9];
    stdout.readExact(&mut output[..]).exit().?;
    stdout.close().exit().?;

    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(5)!;
        },
    };
    if not term.succeeded() {
        return process::exit(6)!;
    }

    let expected: [9]u8 = [b'r', b'o', b'u', b'n', b'd', b't', b'r', b'i', b'p'];
    let mut index = 0usize;
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

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process pipe stdin stdout");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process pipe stdin stdout executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/cat"), init.env())
        .withStdin(process::StdIo::Pipe)
        .withStdout(process::StdIo::Ignore);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    if not term.succeeded() {
        return process::exit(4)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process wait closes stdin");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process wait closes stdin executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/true"), init.env());
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let first = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    let second = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(4)!;
        },
    };
    if not first.succeeded() or not second.succeeded() {
        return process::exit(5)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process repeat wait");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process repeat wait executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/true"), init.env());
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let mut spins = 0usize;
    while spins < 100000usize {
        let maybe = switch child.tryWait() {
            !value => {
                value
            },
            error! => {
                return process::exit(3)!;
            },
        };
        switch maybe {
            ?term => {
                if not term.succeeded() {
                    return process::exit(4)!;
                }
                let again = switch child.tryWait() {
                    !value => {
                        value
                    },
                    error! => {
                        return process::exit(5)!;
                    },
                };
                switch again {
                    ?cached => {
                        if not cached.succeeded() {
                            return process::exit(6)!;
                        }
                    },
                    null => {
                        return process::exit(7)!;
                    },
                }
                return !{};
            },
            null => {},
        }
        spins += 1usize;
    }
    return process::exit(8)!;
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process try wait");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process try wait executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/cat"), init.env())
        .withStdin(process::StdIo::Pipe)
        .withStdout(process::StdIo::Ignore);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let first = switch child.tryWait() {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    switch first {
        ?term => {
            _ = term;
            return process::exit(4)!;
        },
        null => {},
    }
    child.closeStdin().exit().?;
    let term = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(5)!;
        },
    };
    if not term.succeeded() {
        return process::exit(6)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process try wait keeps stdin");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe).output_timeout_without_resources(
        "run emitted std process try wait keeps stdin executable",
    );
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
    let arguments: [2]&[char] = [&"-c", &"while true; do sleep 1; done"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStdout(process::StdIo::Ignore)
        .withStderr(process::StdIo::Ignore);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    let term = switch child.kill() {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    let signal = switch term.signalCode() {
        ?value => {
            value
        },
        null => {
            return process::exit(4)!;
        },
    };
    if signal != 15 {
        return process::exit(5)!;
    }
    let cached = switch child.wait() {
        !value => {
            value
        },
        error! => {
            return process::exit(6)!;
        },
    };
    let cached_signal = switch cached.signalCode() {
        ?value => {
            value
        },
        null => {
            return process::exit(7)!;
        },
    };
    if cached_signal != 15 {
        return process::exit(8)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process kill");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process kill executable");
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
    let arguments: [2]&[char] = [&"-c", &"while true; do sleep 1; done"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withStdout(process::StdIo::Ignore)
        .withStderr(process::StdIo::Ignore);
    let mut child = switch command.spawn() {
        !value => {
            value
        },
        error! => {
            return process::exit(2)!;
        },
    };
    switch child.killWith(999i32 as process::Signal) {
        !value => {
            _ = value;
            return process::exit(10)!;
        },
        process::Error::Kill(process::SystemError::Invalid)! => {},
        error! => {
            _ = error;
            return process::exit(11)!;
        },
    }
    let term = switch child.killWith(process::Signal::Kill) {
        !value => {
            value
        },
        error! => {
            return process::exit(3)!;
        },
    };
    let signal = switch term.signalCode() {
        ?value => {
            value
        },
        null => {
            return process::exit(4)!;
        },
    };
    if signal != 9 {
        return process::exit(5)!;
    }
    let cached = switch child.tryWait() {
        !value => {
            value
        },
        error! => {
            return process::exit(6)!;
        },
    };
    switch cached {
        ?cached_term => {
            let cached_signal = switch cached_term.signalCode() {
                ?value => {
                    value
                },
                null => {
                    return process::exit(7)!;
                },
            };
            if cached_signal != 9 {
                return process::exit(8)!;
            }
        },
        null => {
            return process::exit(9)!;
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process kill with signal");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process kill with signal executable");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn emit_exe_std_process_command_can_set_cwd() {
    let root = temp_dir("emit_exe_std_process_command_can_set_cwd");
    let child_dir = root.join("child-cwd");
    std::fs::create_dir(&child_dir).expect("create child cwd");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let cwd_literal = child_dir.display().to_string();
    std::fs::write(
        &main,
        format!(
            r#"
using std;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {{
    let arguments: [2]&[char] = [&"-c", &"test \"$(basename \"$PWD\")\" = child-cwd"];
    let command = process::Command::init(std::PathView::init(&"/bin/sh"), init.env())
        .withArguments(&arguments)
        .withCwd(std::PathView::init(&"{cwd_literal}"));
    let term = switch command.run() {{
        !value => {{
            value
        }},
        error! => {{
            return process::exit(3)!;
        }},
    }};
    if not term.succeeded() {{
        return process::exit(4)!;
    }}
    !{{}}
}}
"#,
            cwd_literal = cwd_literal
        ),
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process cwd");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process cwd executable");
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
    let command = process::Command::init(std::PathView::init(&"/bin/true"), init.env())
        .withCwd(std::PathView::init(&"/definitely/not/a/nia/process-test-cwd"));
    let child = switch command.spawn() {
        !value => {
            value
        },
        process::Error::Spawn(process::SpawnError::Cwd(process::SystemError::NotFound))! => return !{},
        error! => {
            _ = error;
            return process::exit(3)!;
        },
    };
    _ = child;
    return process::exit(4)!;
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std process cwd stage");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let output = Command::new(&exe)
        .output_timeout_without_resources("run emitted std process cwd stage executable");
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
    let mut paths = args.skipProgram();
    while true {
        let path = switch paths.next() {
            ?value => {
                value
            },
            null => {
                break;
            },
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

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe discarded indirect return call");
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

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe extension helper");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted extension helper executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_std_root_text_and_cstring() {
    let root = temp_dir("emit_exe_exposes_std_root_text_and_cstring");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::fmt;
using std::mem;
using std::process;

struct FailFreeAllocator {
    backing: mem::PageAllocator,
    failNextFree: bool,
}

extend FailFreeAllocator {
    fn init() FailFreeAllocator {
        { backing: mem::PageAllocator::init(), failNextFree: false }
    }

    fn failNext(&mut self) void {
        self.failNextFree = true;
    }
}

extend FailFreeAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        self.backing.free(block).?;
        if self.failNextFree and not block.isEmpty() {
            self.failNextFree = false;
            return mem::Error::Invalid!;
        }
        !{}
    }
}

fn hasUtf8Error(
    result: std::TextError!std::String,
    expected: std::unicode::Utf8DecodeError,
) bool {
    switch result {
        !value => {
            _ = value;
            false
        },
        std::TextError::InvalidUtf8(actual)! => actual == expected,
        std::TextError::Allocation(error)! => {
            _ = error;
            false
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut page_allocator = mem::PageAllocator::init();
    let page: &mut mem::Allocator = &mut page_allocator;

    if std::CStringView::fromBytes(&b"nia\0") is !value {
        if value.len() != 3 {
            return process::exit(1)!;
        }
        let bytes = value.bytes();
        if bytes[0] != b'n' or bytes[1] != b'i' or bytes[2] != b'a' {
            return process::exit(2)!;
        }
    } else {
        return process::exit(3)!;
    }
    switch std::CStringView::fromBytes(&b"nia") {
        !invalid => {
            _ = invalid;
            return process::exit(4)!;
        },
        std::CStringError::MissingTerminator! => {},
        error! => {
            return process::exit(8)!;
        },
    }
    switch std::CStringView::fromBytes(&b"") {
        !invalid => {
            _ = invalid;
            return process::exit(9)!;
        },
        std::CStringError::EmptyInput! => {},
        error! => {
            return process::exit(10)!;
        },
    }
    switch std::CStringView::fromBytes(&b"n\0ia\0") {
        !invalid => {
            _ = invalid;
            return process::exit(11)!;
        },
        std::CStringError::InteriorNul! => {},
        error! => {
            return process::exit(12)!;
        },
    }
    if std::CStringView::fromBytes(&b"\0") is !empty {
        if not empty.isEmpty() {
            return process::exit(13)!;
        }
    } else {
        return process::exit(14)!;
    }
    let cstr_bytes = b"nia\0";
    let ptr = (&cstr_bytes).ptr();
    if ptr[0] != b'n' or ptr[1] != b'i' or ptr[2] != b'a' or ptr[3] != 0u8 {
        return process::exit(5)!;
    }

    let mut ownedCstring = std::CString::fromBytes(page, &b"owned").exit().?;
    defer ownedCstring.deinit(page).exit().?;
    if ownedCstring.len() != 5
        or ownedCstring.bytes()[0] != b'o'
        or ownedCstring.rawPtr()[5] != 0
        or ownedCstring.view().len() != 5
    {
        return process::exit(51)!;
    }
    let mut emptyCstring = std::CString::init(page).exit().?;
    defer emptyCstring.deinit(page).exit().?;
    if not emptyCstring.isEmpty() or emptyCstring.nulTerminatedBytes().len() != 1 {
        return process::exit(58)!;
    }
    let borrowedCstring = std::CStringView::fromPtrUnchecked((&b"copy\0").ptr());
    let mut copiedCstring = std::CString::fromView(page, borrowedCstring).exit().?;
    defer copiedCstring.deinit(page).exit().?;
    if not copiedCstring.view().bytes().equals(&b"copy") {
        return process::exit(52)!;
    }
    let mut unicodeCstring = std::CString::fromText(page, &"nia λ").exit().?;
    defer unicodeCstring.deinit(page).exit().?;
    if unicodeCstring.bytes().len() != 6 or unicodeCstring.rawPtr()[5] != 0xbbu8 {
        return process::exit(53)!;
    }
    switch std::CString::fromBytes(page, &b"bad\0input") {
        !value => {
            _ = value;
            return process::exit(54)!;
        },
        std::CStringBuildError::ContainsNul! => {},
        error! => {
            _ = error;
            return process::exit(55)!;
        },
    }
    switch std::CString::fromText(page, &"bad\0input") {
        !value => {
            _ = value;
            return process::exit(59)!;
        },
        std::CStringBuildError::ContainsNul! => {},
        error! => {
            _ = error;
            return process::exit(60)!;
        },
    }
    let mut cstringTiny: [1]u8 = [0];
    let mut cstringFixed = mem::FixedBufferAllocator::init(&mut cstringTiny[..]);
    switch std::CString::fromText(&mut cstringFixed, &"out") {
        !value => {
            _ = value;
            return process::exit(56)!;
        },
        std::CStringBuildError::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(57)!;
        },
    }

    let utf8: [6]u8 = [b'n', b'i', b'a', b' ', 0xceu8, 0xbbu8];
    if std::String::fromUtf8(page, &utf8) is !value {
        let mut decoded = value;
        defer decoded.deinit(page).exit().?;
        let expected = "nia lambda";
        let mut index = 0usize;
        for &ch in decoded.text().iter() {
            if index < 4usize {
                if ch != expected[index] {
                    return process::exit(15)!;
                }
            } else if ch.codepoint() != 0x03bbu32 {
                return process::exit(16)!;
            }
            index += 1usize;
        }
        if index != 5usize {
            return process::exit(17)!;
        }
    } else {
        return process::exit(18)!;
    }

    if std::String::fromUtf8(page, &b"") is !empty {
        let mut emptyText = empty;
        defer emptyText.deinit(page).exit().?;
        if emptyText.text().len() != 0usize {
            return process::exit(19)!;
        }
    } else {
        return process::exit(20)!;
    }

    let truncated: [2]u8 = [0xe2u8, 0x82u8];
    if not hasUtf8Error(
        std::String::fromUtf8(page, &truncated),
        std::unicode::Utf8DecodeError::Truncated,
    ) {
        return process::exit(21)!;
    }
    let invalidLeading: [1]u8 = [0xffu8];
    if not hasUtf8Error(
        std::String::fromUtf8(page, &invalidLeading),
        std::unicode::Utf8DecodeError::InvalidLeadingByte,
    ) {
        return process::exit(22)!;
    }
    let invalidContinuation: [3]u8 = [0xe2u8, 0x28u8, 0xa1u8];
    if not hasUtf8Error(
        std::String::fromUtf8(page, &invalidContinuation),
        std::unicode::Utf8DecodeError::InvalidContinuation,
    ) {
        return process::exit(23)!;
    }
    let overlong: [2]u8 = [0xc0u8, 0x80u8];
    if not hasUtf8Error(
        std::String::fromUtf8(page, &overlong),
        std::unicode::Utf8DecodeError::Overlong,
    ) {
        return process::exit(24)!;
    }
    let invalidScalar: [3]u8 = [0xedu8, 0xa0u8, 0x80u8];
    if not hasUtf8Error(
        std::String::fromUtf8(page, &invalidScalar),
        std::unicode::Utf8DecodeError::InvalidScalar,
    ) {
        return process::exit(25)!;
    }

    let mut tiny: [1]u8 = [0];
    let mut fixed = mem::FixedBufferAllocator::init(&mut tiny[..]);
    switch std::String::fromUtf8(&mut fixed, &b"nia") {
        !value => {
            _ = value;
            return process::exit(26)!;
        },
        std::TextError::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(27)!;
        },
    }

    let mut text = std::String::fromSlice(page, &"nia").exit().?;
    defer text.deinit(page).exit().?;
    text.append(page, &" std").exit().?;
    if text.text().len() != 7usize {
        return process::exit(6)!;
    }

    let suffix: [3]u8 = [b' ', 0xceu8, 0xbbu8];
    switch text.appendUtf8(page, &suffix) {
        !ok => { _ = ok; },
        error! => {
            _ = error;
            return process::exit(28)!;
        },
    }
    if text.text().len() != 9usize or text.text()[8].codepoint() != 0x03bbu32 {
        return process::exit(29)!;
    }

    let invalidSuffix: [4]u8 = [b'x', 0xe2u8, 0x28u8, 0xa1u8];
    switch text.appendUtf8(page, &invalidSuffix) {
        !ok => {
            _ = ok;
            return process::exit(30)!;
        },
        std::TextError::InvalidUtf8(std::unicode::Utf8DecodeError::InvalidContinuation)! => {},
        error! => {
            _ = error;
            return process::exit(31)!;
        },
    }
    if text.text().len() != 9usize or text.text()[8].codepoint() != 0x03bbu32 {
        return process::exit(32)!;
    }

    let mut appendStorage: [12]u8 = [0; 12];
    let mut appendAllocator = mem::FixedBufferAllocator::init(&mut appendStorage[..]);
    let mut boundedText = switch std::String::fromUtf8(&mut appendAllocator, &b"ok") {
        !value => value,
        error! => {
            _ = error;
            return process::exit(33)!;
        },
    };
    defer boundedText.deinit(&mut appendAllocator).exit().?;
    switch boundedText.appendUtf8(&mut appendAllocator, &b"!") {
        !ok => {
            _ = ok;
            return process::exit(34)!;
        },
        std::TextError::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(35)!;
        },
    }
    if boundedText.text().len() != 2usize
        or boundedText.text()[0] != 'o'
        or boundedText.text()[1] != 'k'
    {
        return process::exit(36)!;
    }

    let answer = 42;
    let formattedScalar = text.text()[8];
    let formatArgs: [2]&fmt::Format = [&answer, &formattedScalar];
    switch text.appendFormat(page, &" value={} {}", &formatArgs) {
        !ok => { _ = ok; },
        error! => {
            _ = error;
            return process::exit(37)!;
        },
    }
    if text.text().len() != 20usize
        or text.text()[8].codepoint() != 0x03bbu32
        or text.text()[16] != '4'
        or text.text()[17] != '2'
        or text.text()[19].codepoint() != 0x03bbu32
    {
        return process::exit(38)!;
    }

    switch text.appendFormat(page, &"partial {", &[]) {
        !ok => {
            _ = ok;
            return process::exit(39)!;
        },
        std::TextFormatError::Format(fmt::Error::InvalidTemplate)! => {},
        error! => {
            _ = error;
            return process::exit(40)!;
        },
    }
    if text.text().len() != 20usize or text.text()[19].codepoint() != 0x03bbu32 {
        return process::exit(41)!;
    }

    let invalidFormattedBytes: [1]u8 = [0xffu8];
    let invalidFormatArgs: [1]&fmt::Format = [&invalidFormattedBytes[..]];
    switch text.appendFormat(page, &"{}", &invalidFormatArgs) {
        !ok => {
            _ = ok;
            return process::exit(42)!;
        },
        std::TextFormatError::InvalidUtf8(std::unicode::Utf8DecodeError::InvalidLeadingByte)! => {},
        error! => {
            _ = error;
            return process::exit(43)!;
        },
    }
    if text.text().len() != 20usize or text.text()[19].codepoint() != 0x03bbu32 {
        return process::exit(44)!;
    }

    let boundedFormatArgs: [1]&fmt::Format = [&answer];
    switch boundedText.appendFormat(&mut appendAllocator, &"{}", &boundedFormatArgs) {
        !ok => {
            _ = ok;
            return process::exit(45)!;
        },
        std::TextFormatError::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::exit(46)!;
        },
    }
    if boundedText.text().len() != 2usize
        or boundedText.text()[0] != 'o'
        or boundedText.text()[1] != 'k'
    {
        return process::exit(47)!;
    }

    let mut cleanupAllocator = FailFreeAllocator::init();
    let mut cleanupText = std::String::initCapacity(&mut cleanupAllocator, 32usize).exit().?;
    defer cleanupText.deinit(&mut cleanupAllocator).exit().?;
    cleanupText.append(&mut cleanupAllocator, &"base").exit().?;
    cleanupAllocator.failNext();
    let cleanupFormatArgs: [1]&fmt::Format = [&answer];
    switch cleanupText.appendFormat(&mut cleanupAllocator, &" {}", &cleanupFormatArgs) {
        !ok => {
            _ = ok;
            return process::exit(48)!;
        },
        std::TextFormatError::Allocation(mem::Error::Invalid)! => {},
        error! => {
            _ = error;
            return process::exit(49)!;
        },
    }
    if not cleanupText.equals(&"base") {
        return process::exit(50)!;
    }

    let mut path = std::PathBuf::fromView(page, std::PathView::init(&"root")).exit().?;
    defer path.deinit(page).exit().?;
    path.joinComponent(page, &"child").exit().?;
    if path.view().text().len() != 10usize {
        return process::exit(7)!;
    }
    _ = init;
    !{}
}
"#,
    )
    .expect("write test source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std root text and CStringView");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let status =
        Command::new(&exe).status_timeout("run emitted std root text and CStringView executable");
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
    process::exit(7)!
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
        .output_timeout_for_build("run nia emit --exe");

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
    let mut values: [3]i32 = [1, 2, 3];
    process::exit(read(&values[..]))!
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::fmt;
using std::io;
using std::process;

pub fn write(init: process::Init) process::ExitCode!void {
    let mut buffer = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(&mut buffer);
    defer stdout.flush().exit().?;
    let text = b"left";
    stdout.print(&"{}\n", &[&text[..]]).exit()
}
"#,
    )
    .expect("write left source");
    std::fs::write(
        &right,
        r#"
using std::fmt;
using std::io;
using std::process;

pub fn write(init: process::Init) process::ExitCode!void {
    let mut buffer = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(&mut buffer);
    defer stdout.flush().exit().?;
    let text = b"right";
    stdout.print(&"{}\n", &[&text[..]]).exit()
}
"#,
    )
    .expect("write right source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

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
    let mut values: [3]i32 = [1, 2, 3];
    process::exit(read(&values[..]))!
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
        .output_timeout_for_build("run nia emit --exe");

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
        return process::exit(1)!;
    }
    if r >= by {
        return process::exit(2)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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
        return process::exit(1)!;
    }
    if r0 < 0i128 or r0 >= divisor {
        return process::exit(2)!;
    }

    let neg_base = -base;
    let q1 = neg_base / divisor;
    let r1 = neg_base % divisor;
    if q1 * divisor + r1 != neg_base {
        return process::exit(3)!;
    }
    if r1 > 0i128 or r1 <= -divisor {
        return process::exit(4)!;
    }

    let neg_divisor = -divisor;
    let q2 = base / neg_divisor;
    let r2 = base % neg_divisor;
    if q2 * neg_divisor + r2 != base {
        return process::exit(5)!;
    }
    if r2 < 0i128 or r2 >= divisor {
        return process::exit(6)!;
    }

    let q3 = neg_base / neg_divisor;
    let r3 = neg_base % neg_divisor;
    if q3 * neg_divisor + r3 != neg_base {
        return process::exit(7)!;
    }
    if r3 > 0i128 or r3 <= -divisor {
        return process::exit(8)!;
    }

    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::mem;
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
    if (fs::Error::NotFound.asExitCode() as i32) != 2 {
        return exit(3)!;
    }
    if (process::Error::Allocation(mem::Error::OutOfMemory).asExitCode() as i32) != 12 {
        return exit(5)!;
    }
    if (process::Error::Path(fs::PathError::TooLong).asExitCode() as i32) != 36 {
        return exit(6)!;
    }
    if (process::Error::ArgumentContainsNul(3).asExitCode() as i32) != 22 {
        return exit(7)!;
    }
    if (process::Error::Environment {
        index: 2,
        cause: process::EnvEntryError::EmptyName,
    }.asExitCode() as i32) != 22 {
        return exit(10)!;
    }
    if (process::Error::Spawn(process::SpawnError::Exec(process::SystemError::NotFound)).asExitCode() as i32) != 2 {
        return exit(8)!;
    }
    if (process::Error::Kill(process::SystemError::Invalid).asExitCode() as i32) != 22 {
        return exit(9)!;
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

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

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
    let mut writer = io::DiscardingWriter::init();
    switch writer.writeAll(&b"nia") {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if writer.len() != 3 {
        return process::exit(2)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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
        return process::exit(1)!;
    }
    if not 4096usize.is_power_of_two() {
        return process::exit(2)!;
    }
    switch 10usize.checked_add(5usize) {
        ?value => { if value != 15usize {
                    return process::exit(3)!;
                } },
        null => { return process::exit(4)!; },
    }
    switch 18446744073709551615usize.checked_add(1usize) {
        ?value => { _ = value;
                return process::exit(5)!; },
        null => { },
    }
    switch 12usize.checked_mul(3usize) {
        ?value => { if value != 36usize {
                    return process::exit(6)!;
                } },
        null => { return process::exit(7)!; },
    }
    switch 4611686018427387904usize.checked_mul(4usize) {
        ?value => { _ = value;
                return process::exit(8)!; },
        null => { },
    }
    switch 17usize.align_forward(8usize) {
        ?value => { if value != 24usize {
                    return process::exit(9)!;
                } },
        null => { return process::exit(10)!; },
    }
    switch 17usize.align_forward(3usize) {
        ?value => { _ = value;
                return process::exit(11)!; },
        null => { },
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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

    switch add_checked_same[u8](250u8, 5u8) {
        ?value => { if value != 255u8 { return process::exit(1)!; } },
        null => { return process::exit(2)!; },
    }
    switch 255u8.checked_add(1u8) {
        ?value => { _ = value; return process::exit(3)!; },
        null => { },
    }
    switch 10u16.checked_sub(3u16) {
        ?value => { if value != 7u16 { return process::exit(4)!; } },
        null => { return process::exit(5)!; },
    }
    switch 0u16.checked_sub(1u16) {
        ?value => { _ = value; return process::exit(6)!; },
        null => { },
    }
    switch 70000u32.checked_mul(60000u32) {
        ?value => { if value != 4200000000u32 { return process::exit(7)!; } },
        null => { return process::exit(8)!; },
    }
    switch 0xffffffffu32.checked_mul(2u32) {
        ?value => { _ = value; return process::exit(9)!; },
        null => { },
    }
    switch 100u64.checked_div(4u64) {
        ?value => { if value != 25u64 { return process::exit(10)!; } },
        null => { return process::exit(11)!; },
    }
    switch 100u64.checked_div(0u64) {
        ?value => { _ = value; return process::exit(12)!; },
        null => { },
    }
    switch 100u128.checked_rem(7u128) {
        ?value => { if value != 2u128 { return process::exit(13)!; } },
        null => { return process::exit(14)!; },
    }
    switch 100u128.checked_rem(0u128) {
        ?value => { _ = value; return process::exit(15)!; },
        null => { },
    }
    switch 9usize.checked_sub(4usize) {
        ?value => { if value != 5usize { return process::exit(16)!; } },
        null => { return process::exit(17)!; },
    }

    switch (-5i8).checked_neg() {
        ?value => { if value != 5i8 { return process::exit(18)!; } },
        null => { return process::exit(19)!; },
    }
    switch i8::MIN.checked_neg() {
        ?value => { _ = value; return process::exit(20)!; },
        null => { },
    }
    switch (-123i16).checked_abs() {
        ?value => { if value != 123i16 { return process::exit(21)!; } },
        null => { return process::exit(22)!; },
    }
    switch i16::MIN.checked_abs() {
        ?value => { _ = value; return process::exit(23)!; },
        null => { },
    }
    switch i32::MAX.checked_add(1i32) {
        ?value => { _ = value; return process::exit(24)!; },
        null => { },
    }
    switch (-10i32).checked_add(5i32) {
        ?value => { if value != -5i32 { return process::exit(25)!; } },
        null => { return process::exit(26)!; },
    }
    switch i64::MIN.checked_sub(1i64) {
        ?value => { _ = value; return process::exit(27)!; },
        null => { },
    }
    switch 10i64.checked_sub(-5i64) {
        ?value => { if value != 15i64 { return process::exit(28)!; } },
        null => { return process::exit(29)!; },
    }
    switch i128::MIN.checked_mul(-1i128) {
        ?value => { _ = value; return process::exit(30)!; },
        null => { },
    }
    switch 12i128.checked_mul(-3i128) {
        ?value => { if value != -36i128 { return process::exit(31)!; } },
        null => { return process::exit(32)!; },
    }
    switch isize::MIN.checked_div(-1isize) {
        ?value => { _ = value; return process::exit(33)!; },
        null => { },
    }
    switch (-9isize).checked_div(3isize) {
        ?value => { if value != -3isize { return process::exit(34)!; } },
        null => { return process::exit(35)!; },
    }
    switch (-9isize).checked_rem(0isize) {
        ?value => { _ = value; return process::exit(36)!; },
        null => { },
    }

    !{}
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_process_args_as_values() {
    let root = temp_dir("emit_exe_exposes_process_args_as_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::parse;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut args = init.args();
    if args.isEmpty() {
        return process::exit(23)!;
    }
    if args.len() != 3 {
        return process::exit(1)!;
    }
    let rawArgv = args.rawArgv();
    if rawArgv as usize != init.rawArgv() as usize {
        return process::exit(24)!;
    }
    switch args.program() {
        ?program => { if program.isEmpty() {
                return process::exit(9)!;
            }
            if program.rawPtr() as usize != rawArgv[0] as usize {
                return process::exit(25)!;
            }
        },
        null => { return process::exit(10)!; },
    }
    let mut iter = args.skipProgram();
    if iter.remaining() != 2 {
        return process::exit(11)!;
    }
    let mut first_arg = switch iter.next() {
        ?value => { value },
        null => { return process::exit(2)!; },
    };
    if iter.remaining() != 1 {
        return process::exit(12)!;
    }
    let mut second_arg = switch iter.next() {
        ?value => { value },
        null => { return process::exit(3)!; },
    };
    if iter.remaining() != 0 {
        return process::exit(13)!;
    }
    if first_arg.isEmpty() or second_arg.isEmpty() {
        return process::exit(26)!;
    }
    if first_arg.rawPtr() as usize != rawArgv[1] as usize or second_arg.rawPtr() as usize != rawArgv[2] as usize {
        return process::exit(27)!;
    }
    let mut for_count = 0;
    for arg in args.skipProgram() {
        if for_count == 0 {
            if arg.len() != 3 {
                return process::exit(18)!;
            }
        } else if for_count == 1 {
            switch arg.parse[u16]() {
                !value => {
                    if value != 1234 {
                        return process::exit(19)!;
                    }
                },
                error! => {
                    return process::exit(20)!;
                },
            }
        } else {
            return process::exit(21)!;
        }
        for_count += 1;
    }
    if for_count != 2 {
        return process::exit(22)!;
    }
    let mut first = first_arg.bytes();
    let mut second = second_arg.bytes();
    if first.len() != 3 {
        return process::exit(4)!;
    }
    if first[0] != 110u8 or first[1] != 105u8 or first[2] != 97u8 {
        return process::exit(5)!;
    }
    if second.len() != 4 {
        return process::exit(6)!;
    }
    switch second_arg.parse[u16]() {
        !value => { if value != 1234 {
                return process::exit(14)!;
            } },
        error! => { return process::exit(15)!; },
    }
    switch second_arg.parseRadix[u16](16) {
        !value => { if value != 0x1234 {
                return process::exit(16)!;
            } },
        error! => { return process::exit(17)!; },
    }
    let mut storage: [16]u8 = [0; 16];
    let mut writer = io::FixedBufferWriter::init(&mut storage[..]);
    writer.print(&"{:_>5.2}", &[&first_arg]).exit().?;
    let written = writer.written();
    if written.len() != 5 or written[0] != b'_' or written[1] != b'_' or written[2] != b'_' or written[3] != b'n' or written[4] != b'i' {
        return process::exit(8)!;
    }
    switch iter.next() {
        ?value => { _ = value;
                return process::exit(7)!; },
        null => { },
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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
    let needle: &[u8] = &b"NIA_TEST_ENV=ok";
    if bytes.len() < needle.len() {
        return false;
    }
    let mut index: usize = 0;
    while index < needle.len() {
        if bytes[index] != needle[index] {
            return false;
        }
        index += 1;
    }
    true
}

pub fn main(init: process::Init) process::ExitCode!void {
    let env = init.env();
    let rawEnvp = env.rawEnvp();
    if rawEnvp as usize != init.rawEnvp() as usize {
        return process::exit(3)!;
    }
    let mut index: usize = 0;
    for item in env.iter() {
        if starts_with_needle(item.bytes()) {
            if item.isEmpty() {
                return process::exit(4)!;
            }
            if item.rawPtr() as usize != rawEnvp[index] as usize {
                return process::exit(5)!;
            }
            return !{};
        }
        index += 1;
    }
    return process::exit(2)!;
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
        .output_timeout_for_build("run nia emit --exe");

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
        switch self {
            !value => { !value },
            err! => { map_parse_error(err)! },
        }
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch parse().as_app_error() {
        !value => { return process::exit(value)!; },
        err! => { return process::exit(err as i32)!; },
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
        .output_timeout_for_build("run nia emit --exe");

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
    let mut left = 20;
    let mut right = 22;

    let &x = &left;
    let mut &mut y: i32 = &mut right;
    y += 1;

    if x + y != 43 {
        return process::exit(1)!;
    }
    if right != 22 {
        return process::exit(2)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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

static mut calls: i32 = 0;

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
    switch value {
        ?!ok => {
            ok
        },
        ?err! => {
            err
        },
        null => {
            0
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut total = 0;
    switch next(true) {
        ?!value => {
            total = value;
        },
        ?err! => {
            total = err + 10;
        },
        null => {
            total = 20;
        },
    }
    if calls != 1 {
        return process::exit(1)!;
    }
    if total != 7 {
        return process::exit(2)!;
    }
    if classify(next(false)) != 5 {
        return process::exit(3)!;
    }
    if calls != 2 {
        return process::exit(4)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

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
    let mut counter = Counter::init();
    counter.add(7);
    if counter.get() != 7 {
        return process::exit(1)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_generic_local_static_has_per_instance_storage() {
    let root = temp_dir("emit_exe_generic_local_static_has_per_instance_storage");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fn slot[T]() &mut T {
    static mut item: T;
    &mut item
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut left = slot[i32]();
    let mut right = slot[u64]();
    left.* = 11;
    right.* = 99u64;

    let mut left_again = slot[i32]();
    let mut right_again = slot[u64]();
    if left_again.* != 11 {
        return process::exit(1)!;
    }
    if right_again.* != 99u64 {
        return process::exit(2)!;
    }

    left_again.* = 7;
    if slot[i32]().* != 7 {
        return process::exit(3)!;
    }
    if slot[u64]().* != 99u64 {
        return process::exit(4)!;
    }
    !{}
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
