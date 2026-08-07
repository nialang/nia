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
        arch,
        vendor,
        os,
        env,
        abi,
        endian,
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
        build::OptimizationMode::O0,
        1u32,
        1usize,
    )
}

fn textIs(actual: &[char], expected: &[char]) bool {
    actual.equals(expected)
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

fn rejectsInvalidPackage(result: build::Error!build::PackageHandle, index: usize) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Package(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsForeignPackageInput(result: build::Error!build::StepHandle) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Packages,
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignPackageModule(result: build::Error!build::ModuleHandle) bool {
    switch result {
        !handle => {
            _ = handle;
            false
        },
        error! => switch error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Packages,
            } => true,
            _ => false,
        },
    }
}

fn rejectsInvalidPlanModule(result: build::Error!void, index: usize) bool {
    switch result {
        !ok => {
            _ = ok;
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
        return process::exit(1)!;
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
        return process::exit(8)!;
    }
    if not rejectsInvalidStep(api.addAggregateStep(&"bad name"), 1usize) {
        return process::exit(9)!;
    }
    if not rejectsInvalidStep(
        api.addGeneratedFileStep(
            &"generate",
            build::BuildPathView::init(&"../escape"),
            &b"contents"[..],
        ),
        1usize,
    ) {
        return process::exit(11)!;
    }
    let invalidArgument: [3]char = ['a', '\0', 'b'];
    let invalidArguments: [1]&[char] = [&invalidArgument];
    if not rejectsInvalidStep(
        api.addRunExecutableStep(
            &"invalid-run",
            build::RunOptions::init(executable).withArguments(&invalidArguments[..]),
        ),
        1usize,
    ) {
        return process::exit(13)!;
    }
    let commandOutputs = [
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"first.out")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"second.out")),
    ];
    _ = api.addExternalCommandStep(
        &"multi-output-tool",
        build::ExternalCommandOptions::search(&"tool").withArguments(&commandOutputs),
    ).exit().?;
    if not rejectsInvalidStep(
        api.addExternalCommandStep(
            &"invalid-cwd",
            build::ExternalCommandOptions::search(&"tool")
                .withPackageWorkingDirectory(fs::PathView::init(&"../escape")),
        ),
        2usize,
    ) {
        return process::exit(15)!;
    }
    let duplicateImports = [
        build::ModuleImport::init(&"dep", fs::PathView::init(&"first.nia")),
        build::ModuleImport::init(&"dep", fs::PathView::init(&"second.nia")),
    ];
    if not rejectsDuplicateImport(api.addModule(
        build::ModuleOptions::init(&"duplicate-imports", fs::PathView::init(&"dup.nia"))
            .withImports(&duplicateImports[..]),
    )) {
        return process::exit(10)!;
    }
    if not rejectsInvalidPackage(
        api.addPackage(build::PackageOptions::init(
            &"root",
            fs::PathView::init(&"packages/root"),
        )),
        1usize,
    ) {
        return process::exit(17)!;
    }
    let assets = api.addPackage(build::PackageOptions::init(
        &"assets",
        fs::PathView::init(&"packages/assets"),
    )).exit().?;
    if not rejectsInvalidPackage(
        api.addPackage(build::PackageOptions::init(
            &"assets",
            fs::PathView::init(&"packages/other"),
        )),
        2usize,
    ) or not rejectsInvalidPackage(
        api.addPackage(build::PackageOptions::init(
            &"other",
            fs::PathView::init(&"packages/assets"),
        )),
        2usize,
    ) or not rejectsInvalidPackage(
        api.addPackage(build::PackageOptions::init(
            &"escape",
            fs::PathView::init(&"../escape"),
        )),
        2usize,
    ) {
        return process::exit(18)!;
    }
    let packageArguments = [build::CommandArgument::packageInput(
        assets,
        fs::PathView::init(&"input.txt"),
    )];
    _ = api.addExternalCommandStep(
        &"package-input",
        build::ExternalCommandOptions::search(&"tool").withArguments(&packageArguments),
    ).exit().?;

    let mut other = initBuild(init, &mut allocator).exit().?;
    defer other.deinit().exit().?;
    let otherModule = other.addModule(
        build::ModuleOptions::init(&"other", fs::PathView::init(&"other.nia")),
    ).exit().?;
    let otherExecutable = other.addExecutable(
        build::ExecutableOptions::init(&"other-app", otherModule),
    ).exit().?;
    let otherStep = other.addEmitExecutableStep(&"other-emit", otherExecutable).exit().?;
    let otherPackage = other.addPackage(build::PackageOptions::init(
        &"other-package",
        fs::PathView::init(&"packages/other"),
    )).exit().?;
    if not rejectsForeignPackageModule(api.addModule(build::ModuleOptions::fromPackage(
        &"foreign-package-module",
        otherPackage,
        fs::PathView::init(&"main.nia"),
    ))) {
        return process::exit(20)!;
    }
    let foreignPackageImports = [build::ModuleImport::fromPackage(
        &"foreignPackageImport",
        otherPackage,
        fs::PathView::init(&"helper.nia"),
    )];
    if not rejectsForeignPackageModule(api.addModule(
        build::ModuleOptions::init(&"foreign-package-import", fs::PathView::init(&"main.nia"))
            .withImports(&foreignPackageImports),
    )) {
        return process::exit(21)!;
    }
    let foreignPackageArguments = [build::CommandArgument::packageInput(
        otherPackage,
        fs::PathView::init(&"input.txt"),
    )];
    if not rejectsForeignPackageInput(api.addExternalCommandStep(
        &"foreign-package-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&foreignPackageArguments),
    )) {
        return process::exit(19)!;
    }
    if not rejectsForeignModule(api.addExecutable(
        build::ExecutableOptions::init(&"foreign-module", otherModule),
    )) {
        return process::exit(4)!;
    }
    if not rejectsForeignExecutable(api.addEmitExecutableStep(&"foreign-executable", otherExecutable)) {
        return process::exit(5)!;
    }
    if not rejectsForeignExecutable(api.addRunExecutableStep(
        &"foreign-run",
        build::RunOptions::init(otherExecutable),
    )) {
        return process::exit(12)!;
    }
    let foreignArtifactArguments = [build::CommandArgument::artifactInput(otherExecutable)];
    if not rejectsForeignExecutable(api.addExternalCommandStep(
        &"foreign-artifact-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&foreignArtifactArguments[..]),
    )) {
        return process::exit(14)!;
    }
    if not rejectsForeignStep(api.dependOn(emit, otherStep)) {
        return process::exit(6)!;
    }
    if not rejectsForeignStep(api.setDefaultStep(otherStep)) {
        return process::exit(7)!;
    }

    let mut missingProducer = initBuild(init, &mut allocator).exit().?;
    defer missingProducer.deinit().exit().?;
    let generatedModule = missingProducer.addModule(
        build::ModuleOptions::fromBuild(
            &"generated",
            build::BuildPathView::init(&"generated/root.nia"),
        ),
    ).exit().?;
    let generatedExecutable = missingProducer.addExecutable(
        build::ExecutableOptions::init(&"generated-app", generatedModule),
    ).exit().?;
    let generatedEmit = missingProducer.addEmitExecutableStep(
        &"generated-emit",
        generatedExecutable,
    ).exit().?;
    missingProducer.setDefaultStep(generatedEmit).exit().?;
    if not rejectsInvalidPlanModule(missingProducer.validatePlan(), 0usize) {
        return process::exit(16)!;
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
