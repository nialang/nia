// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

#[allow(unused_imports)]
use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn build_configuration_does_not_execute_compiler_actions() {
    let root = temp_dir("build_configuration_does_not_execute_compiler_actions");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::build;
using std::fs;
using std::mem;
using std::process;

fn target() build::TargetView {
    let empty = "";
    build::TargetView::init(&empty, &empty, &empty, &empty, &empty, &empty, 64u32)
}

fn initBuild(
    init: process::Init,
    allocator: &mut mem::Allocator,
    toolchain: fs::PathView,
) build::Error!build::Build {
    let empty = "";
    let emptyPath = fs::PathView::init(&empty);
    let currentTarget = target();
    build::Build::init(
        init,
        allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        toolchain,
        emptyPath,
        currentTarget,
        currentTarget,
        1u32,
        1024usize,
    )
}

fn addCheck(api: &mut build::Build, source: fs::PathView) build::Error!void {
    let moduleHandle = api.addModule(build::ModuleOptions::init(&"root", source)).?;
    let executable = api.addExecutable(build::ExecutableOptions::init(&"app", moduleHandle)).?;
    let check = api.addCheckExecutableStep(&"check", executable).?;
    api.setDefaultStep(check)
}

fn checkConfigurationDoesNotExecuteCompiler(
    init: process::Init,
    allocator: &mut mem::Allocator,
) process::ExitCode!void {
    let mut api = initBuild(
        init,
        allocator,
        fs::PathView::init(&"/definitely/missing/nia-compiler"),
    ).exit().?;
    defer api.deinit().exit().?;
    addCheck(&mut api, fs::PathView::init(&"main.nia")).exit().?;
    api.validatePlan().exit().?;
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    checkConfigurationDoesNotExecuteCompiler(init, &mut pageAllocator).?;
    !{}
}
"#,
    )
    .expect("write contextual build error source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile contextual build error fixture");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output =
        Command::new(&exe).output_timeout_without_resources("run contextual build error fixture");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_process_errors_preserve_exit_codes_and_diagnostics() {
    let root = temp_dir("build_process_errors_preserve_exit_codes_and_diagnostics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::build;
using std::io;
using std::os;
using std::process;

fn buildError(cause: process::Error) build::Error {
    build::Error::Failure {
        operation: build::ErrorOperation::ExecuteCommand,
        subject: build::ErrorSubject::Compiler,
        cause: build::ErrorCause::Process(cause),
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    let spawn = buildError(process::Error::Spawn(os::SpawnError::Exec));
    if (spawn.asExitCode() as i32) != 104 {
        return process::exit(1)!;
    }
    let close = buildError(process::Error::Close {
        stream: process::StdStream::Stdout,
        cause: os::Error::BadFd,
    });
    if (close.asExitCode() as i32) != 9 {
        return process::exit(2)!;
    }
    let environment = buildError(process::Error::Environment {
        index: 1,
        cause: process::EnvEntryError::DuplicateName(0),
    });
    if (environment.asExitCode() as i32) != 22 {
        return process::exit(3)!;
    }

    let mut buffer: [256]u8 = [_]u8[0; 256];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut buffer);
    stdout.print(&"{}\n{}\n{}\n", &[&spawn, &close, &environment]).exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write process build error source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile process build error fixture");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output =
        Command::new(&exe).output_timeout_without_resources("run process build error fixture");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "execute command compiler: process/spawn/executable\n",
            "execute command compiler: process/close/stdout/bad file descriptor\n",
            "execute command compiler: process/environment[1]/duplicates environment[0]\n",
        )
    );
    assert!(output.stderr.is_empty());
}
