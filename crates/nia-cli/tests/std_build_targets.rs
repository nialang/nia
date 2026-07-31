// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn build_separates_owned_host_and_artifact_targets() {
    let root = temp_dir("build_separates_owned_host_and_artifact_targets");
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

fn target(
    arch: &[char],
    vendor: &[char],
    os: &[char],
    env: &[char],
    abi: &[char],
    endian: &[char],
    pointerWidth: u32,
) build::TargetView {
    build::TargetView::init(
        string::StringView::init(arch),
        string::StringView::init(vendor),
        string::StringView::init(os),
        string::StringView::init(env),
        string::StringView::init(abi),
        string::StringView::init(endian),
        pointerWidth,
    )
}

fn initBuild(init: process::Init, allocator: &mut mem::Allocator) build::Error!build::Build {
    let pathText = "temporary-path";
    let hostArch = "host-arch";
    let hostVendor = "host-vendor";
    let hostOs = "host-os";
    let hostEnv = "host-env";
    let hostAbi = "host-abi";
    let hostEndian = "little";
    let artifactArch = "artifact-arch";
    let artifactVendor = "artifact-vendor";
    let artifactOs = "artifact-os";
    let artifactEnv = "artifact-env";
    let artifactAbi = "artifact-abi";
    let artifactEndian = "big";
    build::Build::init(
        init,
        allocator,
        fs::PathView::init(&pathText),
        fs::PathView::init(&pathText),
        fs::PathView::init(&pathText),
        fs::PathView::init(&pathText),
        fs::PathView::init(&pathText),
        target(&hostArch, &hostVendor, &hostOs, &hostEnv, &hostAbi, &hostEndian, 64u32),
        target(
            &artifactArch,
            &artifactVendor,
            &artifactOs,
            &artifactEnv,
            &artifactAbi,
            &artifactEndian,
            32u32,
        ),
        1u32,
        1usize,
    )
}

fn textIs(actual: string::StringView, expected: &[char]) bool {
    mem::equal[char](actual.text(), expected)
}

fn rejectsForeignModule(result: build::Error!build::ExecutableHandle) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Module(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignExecutable(result: build::Error!build::StepHandle) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Executable(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignStep(result: build::Error!void) bool {
    switch result {
        !ok => {
            _ = ok;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Step(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsInvalidModule(result: build::Error!build::ModuleHandle, index: usize) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Module(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsInvalidStep(result: build::Error!build::StepHandle, index: usize) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Step(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsDuplicateImport(result: build::Error!build::ModuleHandle) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::ModuleImport(1usize),
            } => true,
            _ => false,
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut pageAllocator);
    defer allocator.deinit().ok().exit().?;
    let mut api = initBuild(init, &mut allocator).exit().?;
    defer api.deinit().exit().?;

    let host = api.hostTarget();
    let artifact = api.artifactTarget();
    if host.equals(artifact)
        or not textIs(host.arch(), &"host-arch")
        or not textIs(host.vendor(), &"host-vendor")
        or not textIs(host.os(), &"host-os")
        or not textIs(host.env(), &"host-env")
        or not textIs(host.abi(), &"host-abi")
        or not textIs(host.endian(), &"little")
        or host.pointerWidth() != 64u32
        or not textIs(artifact.arch(), &"artifact-arch")
        or not textIs(artifact.vendor(), &"artifact-vendor")
        or not textIs(artifact.os(), &"artifact-os")
        or not textIs(artifact.env(), &"artifact-env")
        or not textIs(artifact.abi(), &"artifact-abi")
        or not textIs(artifact.endian(), &"big")
        or artifact.pointerWidth() != 32u32
    {
        return (1 as process::ExitCode)!;
    }

    let moduleHandle = api.addModule(
        build::ModuleOptions::init(&"main", fs::PathView::init(&"main.nia")),
    ).exit().?;
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&"app", moduleHandle),
    ).exit().?;
    let emit = api.addEmitExecutableStep(&"emit", executable).exit().?;
    api.setDefaultStep(emit).exit().?;
    if not rejectsInvalidModule(
        api.addModule(build::ModuleOptions::init(
            &"bad name",
            fs::PathView::init(&"bad.nia"),
        )),
        1usize,
    ) {
        return (8 as process::ExitCode)!;
    }
    if not rejectsInvalidStep(api.addAggregateStep(&"bad name"), 1usize) {
        return (9 as process::ExitCode)!;
    }
    let duplicateImports = [
        build::ModuleImport::init(&"dep", fs::PathView::init(&"first.nia")),
        build::ModuleImport::init(&"dep", fs::PathView::init(&"second.nia")),
    ];
    if not rejectsDuplicateImport(api.addModule(
        build::ModuleOptions::init(&"duplicate-imports", fs::PathView::init(&"dup.nia"))
            .withImports(&duplicateImports[..]),
    )) {
        return (10 as process::ExitCode)!;
    }

    let mut other = initBuild(init, &mut allocator).exit().?;
    defer other.deinit().exit().?;
    let otherModule = other.addModule(
        build::ModuleOptions::init(&"other", fs::PathView::init(&"other.nia")),
    ).exit().?;
    let otherExecutable = other.addExecutable(
        build::ExecutableOptions::init(&"other-app", otherModule),
    ).exit().?;
    let otherStep = other.addEmitExecutableStep(&"other-emit", otherExecutable).exit().?;
    if not rejectsForeignModule(api.addExecutable(
        build::ExecutableOptions::init(&"foreign-module", otherModule),
    )) {
        return (4 as process::ExitCode)!;
    }
    if not rejectsForeignExecutable(api.addEmitExecutableStep(&"foreign-executable", otherExecutable)) {
        return (5 as process::ExitCode)!;
    }
    if not rejectsForeignStep(api.dependOn(emit, otherStep)) {
        return (6 as process::ExitCode)!;
    }
    if not rejectsForeignStep(api.setDefaultStep(otherStep)) {
        return (7 as process::ExitCode)!;
    }
    api.validatePlan().exit().?;
    !{}
}
"#,
    )
    .expect("write target conformance source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile build target conformance fixture");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .status_timeout("run build target conformance fixture")
            .code(),
        Some(0)
    );
}
