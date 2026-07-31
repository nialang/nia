// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

#[allow(unused_imports)]
use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn build_errors_preserve_context_and_report_exact_causes() {
    let root = temp_dir("build_errors_preserve_context_and_report_exact_causes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::build;
using std::fs;
using std::mem;
using std::process;
using std::string;

fn target() build::TargetView {
    let empty = "";
    let text = string::StringView::init(&empty);
    build::TargetView::init(text, text, text, text, text, text, 64u32)
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
        false,
        1024usize,
    )
}

fn addCheck(api: &mut build::Build, source: fs::PathView) build::Error!void {
    let moduleHandle = api.addModule(build::ModuleOptions::init(&"root", source)).?;
    let executable = api.addExecutable(build::ExecutableOptions::init(&"app", moduleHandle)).?;
    let check = api.addCheckExecutableStep(&"check", executable).?;
    api.setDefaultStep(check)
}

fn checkMissingCompiler(
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
    switch api.runRequestedStep() {
        !ok => {
            _ = ok;
            return (1 as process::ExitCode)!;
        },
        error! => {
            api.reportError(error).exit().?;
            switch error {
                build::Error::Failure {
                    operation: build::ErrorOperation::ExecuteCommand,
                    subject: build::ErrorSubject::Compiler,
                    cause: build::ErrorCause::Process(process::Error::SpawnExec),
                } => {},
                _ => return (2 as process::ExitCode)!,
            }
        },
    }
    !{}
}

fn checkCompilerExit(
    init: process::Init,
    allocator: &mut mem::Allocator,
) process::ExitCode!void {
    let mut api = initBuild(init, allocator, fs::PathView::init(&"/bin/false")).exit().?;
    defer api.deinit().exit().?;
    addCheck(&mut api, fs::PathView::init(&"main.nia")).exit().?;
    switch api.runRequestedStep() {
        !ok => {
            _ = ok;
            return (3 as process::ExitCode)!;
        },
        error! => {
            api.reportError(error).exit().?;
            switch error {
                build::Error::Failure {
                    operation: build::ErrorOperation::ExecuteCommand,
                    subject: build::ErrorSubject::Compiler,
                    cause: build::ErrorCause::Exit(term),
                } => {
                    if term.exit_code() is ?code {
                        if code != 1 {
                            return (4 as process::ExitCode)!;
                        }
                    } else {
                        return (5 as process::ExitCode)!;
                    }
                },
                _ => return (6 as process::ExitCode)!,
            }
        },
    }
    !{}
}

fn checkInvalidPath(
    init: process::Init,
    allocator: &mut mem::Allocator,
) process::ExitCode!void {
    let mut api = initBuild(init, allocator, fs::PathView::init(&"/bin/false")).exit().?;
    defer api.deinit().exit().?;
    let invalidPath = ['m', '\0'];
    addCheck(&mut api, fs::PathView::init(&invalidPath[..])).exit().?;
    switch api.runRequestedStep() {
        !ok => {
            _ = ok;
            return (7 as process::ExitCode)!;
        },
        error! => {
            api.reportError(error).exit().?;
            switch error {
                build::Error::Failure {
                    operation: build::ErrorOperation::Encode,
                    subject: build::ErrorSubject::Module(0usize),
                    cause: build::ErrorCause::FileSystem(fs::Error::Invalid),
                } => {},
                _ => return (8 as process::ExitCode)!,
            }
        },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    checkMissingCompiler(init, &mut pageAllocator).?;
    checkCompilerExit(init, &mut pageAllocator).?;
    checkInvalidPath(init, &mut pageAllocator).?;
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("build error: execute command compiler: process/spawn executable"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("build error: execute command compiler: command/exit code 1"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("build error: encode module[0]: filesystem/invalid"),
        "stderr:\n{stderr}"
    );
}
