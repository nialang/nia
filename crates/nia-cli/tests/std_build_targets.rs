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
        false,
        1usize,
    )
}

fn textIs(actual: string::StringView, expected: &[char]) bool {
    mem::equal[char](actual.text(), expected)
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
        build::ModuleOptions::init(fs::PathView::init(&"main.nia")),
    ).exit().?;
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&"app", moduleHandle),
    ).exit().?;
    let emit = api.addEmitExecutableStep(&"emit", executable).exit().?;
    api.setDefaultStep(emit).exit().?;
    switch api.runRequestedStep() {
        !ok => {
            _ = ok;
            return (2 as process::ExitCode)!;
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::ArtifactTarget,
            } => {},
            _ => return (3 as process::ExitCode)!,
        },
    }
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
