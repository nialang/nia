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
