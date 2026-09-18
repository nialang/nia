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
    let mut initialization = build::Build::init(
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
        __NIA_BUILD_PLAN_COMPATIBILITY__u32,
        null,
        false,
    );
    initialization.finish()
}

fn textIs(actual: &[char], expected: &[char]) bool {
    actual.equals(expected)
}

fn rejectsForeignModule(result: build::Error!build::ExecutableHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Module(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignExecutable(result: build::Error!build::StepHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Executable(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignObject(result: build::Error!build::StepHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Object(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignStaticArchive(result: build::Error!build::StepHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::StaticArchive(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignStaticArchiveTarget(result: build::Error!build::ExecutableHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::StaticArchive(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsInvalidExecutable(result: build::Error!build::ExecutableHandle, index: usize) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Executable(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsForeignStep(result: build::Error!()) bool {
    match result {
        !ok => {
            _ = ok;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Step(0usize),
            } => true,
            _ => false,
        },
    }
}

fn rejectsInvalidPackage(result: build::Error!build::PackageHandle, index: usize) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Package(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsForeignPackageInput(result: build::Error!build::StepHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Packages,
            } => true,
            _ => false,
        },
    }
}

fn rejectsForeignPackageModule(result: build::Error!build::ModuleHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Packages,
            } => true,
            _ => false,
        },
    }
}

fn rejectsInvalidPlanModule(result: build::Error!(), index: usize) bool {
    match result {
        !ok => {
            _ = ok;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Module(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsInvalidModule(result: build::Error!build::ModuleHandle, index: usize) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Module(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsInvalidStep(result: build::Error!build::StepHandle, index: usize) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::Step(actual),
            } => actual == index,
            _ => false,
        },
    }
}

fn rejectsDuplicateImport(result: build::Error!build::ModuleHandle) bool {
    match result {
        !handle => {
            _ = handle;
            false
        },
        error! => match error {
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::ModuleImport(1usize),
            } => true,
            _ => false,
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    let mut pageAllocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut pageAllocator);
    defer allocator.deinit().ok().?;
    let mut api = initBuild(init, &mut allocator).?;
    defer api.deinit().?;

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
        return process::ExitCode(1)!;
    }

    let moduleHandle = api.addModule(
        build::ModuleOptions::init(&"main", fs::PathView::init(&"main.nia")),
    ).?;
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&"app", moduleHandle),
    ).?;
    let emit = api.addEmitExecutableStep(&"emit", executable).?;
    api.setDefaultStep(emit).?;
    if not rejectsInvalidModule(
        api.addModule(build::ModuleOptions::init(
            &"bad name",
            fs::PathView::init(&"bad.nia"),
        )),
        1usize,
    ) {
        return process::ExitCode(8)!;
    }
    if not rejectsInvalidStep(api.addAggregateStep(&"bad name"), 1usize) {
        return process::ExitCode(9)!;
    }
    if not rejectsInvalidStep(
        api.addGeneratedFileStep(
            &"generate",
            build::BuildPathView::init(&"../escape"),
            &b"contents"[..],
        ),
        1usize,
    ) {
        return process::ExitCode(11)!;
    }
    if not rejectsInvalidStep(
        api.addInstallExecutableStep(
            &"invalid-install",
            executable,
            build::BuildPathView::init(&"../escape"),
        ),
        1usize,
    ) {
        return process::ExitCode(23)!;
    }
    let invalidArgument: [char; 3] = ['a', '\0', 'b'];
    let invalidArguments: [&[char]; 1] = [&invalidArgument];
    if not rejectsInvalidStep(
        api.addRunExecutableStep(
            &"invalid-run",
            build::RunOptions::init(executable).withArguments(&invalidArguments[..]),
        ),
        1usize,
    ) {
        return process::ExitCode(13)!;
    }
    let commandOutputs = [
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"first.out")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"second.out")),
    ];
    _ = api.addExternalCommandStep(
        &"multi-output-tool",
        build::ExternalCommandOptions::search(&"tool").withArguments(&commandOutputs),
    ).?;
    if not rejectsInvalidStep(
        api.addExternalCommandStep(
            &"invalid-cwd",
            build::ExternalCommandOptions::search(&"tool")
                .withPackageWorkingDirectory(fs::PathView::init(&"../escape")),
        ),
        2usize,
    ) {
        return process::ExitCode(15)!;
    }
    let duplicateImports = [
        build::ModuleImport::init(&"dep", fs::PathView::init(&"first.nia")),
        build::ModuleImport::init(&"dep", fs::PathView::init(&"second.nia")),
    ];
    if not rejectsDuplicateImport(api.addModule(
        build::ModuleOptions::init(&"duplicate-imports", fs::PathView::init(&"dup.nia"))
            .withImports(&duplicateImports[..]),
    )) {
        return process::ExitCode(10)!;
    }
    if not rejectsInvalidPackage(
        api.addPackage(build::PackageOptions::init(
            &"root",
            fs::PathView::init(&"packages/root"),
        )),
        1usize,
    ) {
        return process::ExitCode(17)!;
    }
    let assets = api.addPackage(build::PackageOptions::init(
        &"assets",
        fs::PathView::init(&"packages/assets"),
    )).?;
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
        return process::ExitCode(18)!;
    }
    let packageArguments = [build::CommandArgument::packageInput(
        assets,
        fs::PathView::init(&"input.txt"),
    )];
    _ = api.addExternalCommandStep(
        &"package-input",
        build::ExternalCommandOptions::search(&"tool").withArguments(&packageArguments),
    ).?;

    let mut other = initBuild(init, &mut allocator).?;
    defer other.deinit().?;
    let otherModule = other.addModule(
        build::ModuleOptions::init(&"other", fs::PathView::init(&"other.nia")),
    ).?;
    let otherExecutable = other.addExecutable(
        build::ExecutableOptions::init(&"other-app", otherModule),
    ).?;
    let otherStep = other.addEmitExecutableStep(&"other-emit", otherExecutable).?;
    let otherObject = other.addObject(
        build::ObjectOptions::init(&"other-objects", otherModule),
    ).?;
    let otherStaticArchive = other.addStaticArchive(
        build::StaticArchiveOptions::init(&"other-archive", otherModule),
    ).?;
    let foreignLinkedArchives = [otherStaticArchive];
    if not rejectsForeignStaticArchiveTarget(api.addExecutable(
        build::ExecutableOptions::init(&"foreign-linked", moduleHandle)
            .withStaticArchives(&foreignLinkedArchives[..]),
    )) {
        return process::ExitCode(30)!;
    }
    let otherPackage = other.addPackage(build::PackageOptions::init(
        &"other-package",
        fs::PathView::init(&"packages/other"),
    )).?;
    if not rejectsForeignPackageModule(api.addModule(build::ModuleOptions::fromPackage(
        &"foreign-package-module",
        otherPackage,
        fs::PathView::init(&"main.nia"),
    ))) {
        return process::ExitCode(20)!;
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
        return process::ExitCode(21)!;
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
        return process::ExitCode(19)!;
    }
    if not rejectsForeignModule(api.addExecutable(
        build::ExecutableOptions::init(&"foreign-module", otherModule),
    )) {
        return process::ExitCode(4)!;
    }
    if not rejectsForeignExecutable(api.addEmitExecutableStep(&"foreign-executable", otherExecutable)) {
        return process::ExitCode(5)!;
    }
    if not rejectsForeignObject(api.addEmitObjectStep(&"foreign-object", otherObject)) {
        return process::ExitCode(25)!;
    }
    if not rejectsForeignStaticArchive(api.addEmitStaticArchiveStep(
        &"foreign-static-archive",
        otherStaticArchive,
    )) {
        return process::ExitCode(27)!;
    }
    if not rejectsForeignExecutable(api.addRunExecutableStep(
        &"foreign-run",
        build::RunOptions::init(otherExecutable),
    )) {
        return process::ExitCode(12)!;
    }
    if not rejectsForeignExecutable(api.addTestExecutableStep(
        &"foreign-test",
        build::RunOptions::init(otherExecutable),
    )) {
        return process::ExitCode(22)!;
    }
    if not rejectsForeignExecutable(api.addInstallExecutableStep(
        &"foreign-install",
        otherExecutable,
        build::BuildPathView::init(&"install/foreign"),
    )) {
        return process::ExitCode(24)!;
    }
    let foreignArtifactArguments = [build::CommandArgument::artifactInput(otherExecutable)];
    if not rejectsForeignExecutable(api.addExternalCommandStep(
        &"foreign-artifact-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&foreignArtifactArguments[..]),
    )) {
        return process::ExitCode(14)!;
    }
    let foreignObjectArguments = [build::CommandArgument::objectInput(otherObject)];
    if not rejectsForeignObject(api.addExternalCommandStep(
        &"foreign-object-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&foreignObjectArguments[..]),
    )) {
        return process::ExitCode(26)!;
    }
    let foreignArchiveArguments = [build::CommandArgument::staticArchiveInput(otherStaticArchive)];
    if not rejectsForeignStaticArchive(api.addExternalCommandStep(
        &"foreign-archive-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&foreignArchiveArguments[..]),
    )) {
        return process::ExitCode(28)!;
    }
    if not rejectsForeignStep(api.dependOn(emit, otherStep)) {
        return process::ExitCode(6)!;
    }
    if not rejectsForeignStep(api.setDefaultStep(otherStep)) {
        return process::ExitCode(7)!;
    }

    let mut missingProducer = initBuild(init, &mut allocator).?;
    defer missingProducer.deinit().?;
    let generatedModule = missingProducer.addModule(
        build::ModuleOptions::fromBuild(
            &"generated",
            build::BuildPathView::init(&"generated/root.nia"),
        ),
    ).?;
    let generatedExecutable = missingProducer.addExecutable(
        build::ExecutableOptions::init(&"generated-app", generatedModule),
    ).?;
    let generatedEmit = missingProducer.addEmitExecutableStep(
        &"generated-emit",
        generatedExecutable,
    ).?;
    missingProducer.setDefaultStep(generatedEmit).?;
    if not rejectsInvalidPlanModule(missingProducer.validatePlan(), 0usize) {
        return process::ExitCode(16)!;
    }
    if not rejectsInvalidStep(api.addRunExecutableStep(
        &"artifact-run",
        build::RunOptions::init(executable),
    ), 3usize) {
        return process::ExitCode(34)!;
    }
    if not rejectsInvalidStep(api.addTestExecutableStep(
        &"artifact-test",
        build::RunOptions::init(executable),
    ), 3usize) {
        return process::ExitCode(35)!;
    }
    let hostExecutable = api.addExecutable(
        build::ExecutableOptions::init(&"host-app", moduleHandle)
            .withOutputName(&"host-tool")
            .forHost(),
    ).?;
    _ = api.addCheckExecutableStep(&"host-check", hostExecutable).?;
    _ = api.addEmitExecutableStep(&"host-emit", hostExecutable).?;
    _ = api.addTestExecutableStep(
        &"host-test",
        build::RunOptions::init(hostExecutable),
    ).?;
    let object = api.addObject(
        build::ObjectOptions::init(&"objects", moduleHandle)
            .withOutputName(&"objects-dir"),
    ).?;
    _ = api.addEmitObjectStep(&"object-emit", object).?;
    let hostObject = api.addObject(
        build::ObjectOptions::init(&"host-objects", moduleHandle)
            .withOutputName(&"host-objects-dir")
            .forHost(),
    ).?;
    _ = api.addEmitObjectStep(&"host-object-emit", hostObject).?;
    let staticArchive = api.addStaticArchive(
        build::StaticArchiveOptions::init(&"archive", moduleHandle),
    ).?;
    let missingArchiveProducerArguments = [
        build::CommandArgument::staticArchiveInput(staticArchive),
    ];
    if not rejectsInvalidStep(api.addExternalCommandStep(
        &"missing-archive-producer",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&missingArchiveProducerArguments[..]),
    ), 8usize) {
        return process::ExitCode(29)!;
    }
    _ = api.addEmitStaticArchiveStep(&"archive-emit", staticArchive).?;
    let linkedArchives = [staticArchive];
    let linkedExecutable = api.addExecutable(
        build::ExecutableOptions::init(&"linked-app", moduleHandle)
            .withStaticArchives(&linkedArchives[..]),
    ).?;
    _ = api.addEmitExecutableStep(&"linked-emit", linkedExecutable).?;
    let hostStaticArchive = api.addStaticArchive(
        build::StaticArchiveOptions::init(&"host-archive", moduleHandle)
            .withOutputName(&"libhost-archive.a")
            .forHost(),
    ).?;
    _ = api.addEmitStaticArchiveStep(&"host-archive-emit", hostStaticArchive).?;
    let duplicateLinkedArchives = [staticArchive, staticArchive];
    if not rejectsInvalidExecutable(api.addExecutable(
        build::ExecutableOptions::init(&"duplicate-linked", moduleHandle)
            .withStaticArchives(&duplicateLinkedArchives[..]),
    ), 3usize) {
        return process::ExitCode(31)!;
    }
    let mismatchedLinkedArchives = [staticArchive];
    if not rejectsInvalidExecutable(api.addExecutable(
        build::ExecutableOptions::init(&"mismatched-linked", moduleHandle)
            .forHost()
            .withStaticArchives(&mismatchedLinkedArchives[..]),
    ), 3usize) {
        return process::ExitCode(32)!;
    }

    let mut missingLinkedProducer = initBuild(init, &mut allocator).?;
    defer missingLinkedProducer.deinit().?;
    let missingLinkedModule = missingLinkedProducer.addModule(
        build::ModuleOptions::init(&"linked", fs::PathView::init(&"linked.nia")),
    ).?;
    let missingLinkedArchive = missingLinkedProducer.addStaticArchive(
        build::StaticArchiveOptions::init(&"linked-archive", missingLinkedModule),
    ).?;
    let missingLinkedArchives = [missingLinkedArchive];
    let missingLinkedExecutable = missingLinkedProducer.addExecutable(
        build::ExecutableOptions::init(&"linked-executable", missingLinkedModule)
            .withStaticArchives(&missingLinkedArchives[..]),
    ).?;
    if not rejectsInvalidStep(missingLinkedProducer.addEmitExecutableStep(
        &"missing-linked-producer",
        missingLinkedExecutable,
    ), 0usize) {
        return process::ExitCode(33)!;
    }
    let objectArguments = [build::CommandArgument::objectInput(object)];
    _ = api.addExternalCommandStep(
        &"object-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&objectArguments[..]),
    ).?;
    let archiveArguments = [build::CommandArgument::staticArchiveInput(staticArchive)];
    _ = api.addExternalCommandStep(
        &"archive-input",
        build::ExternalCommandOptions::search(&"tool")
            .withArguments(&archiveArguments[..]),
    ).?;
    api.writePlanDraft(fs::PathView::init(&"plan.draft")).?;
    !()
}
"#
    .replace(
        "__NIA_BUILD_PLAN_COMPATIBILITY__",
        &nia_compat::formats::BUILD_PLAN
            .release_compatibility
            .to_string(),
    )
    .as_bytes(),
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
            .current_dir(&root)
            .status_timeout("run build target conformance fixture")
            .code(),
        Some(0)
    );
    let plan = nia_build::read_build_plan(&root.join("plan.draft"))
        .expect("decode target conformance plan");
    let artifact_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "emit")
        .expect("artifact emit action");
    assert!(matches!(
        &artifact_emit.kind,
        nia_build::ActionKind::CompilerEmit { target, .. }
            if target == plan.artifact_target()
    ));
    let object_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "object-emit")
        .expect("object emit action");
    assert!(matches!(
        &object_emit.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if target == plan.artifact_target()
                && plan.artifacts().iter().any(|item| {
                    item.key.name() == "objects"
                        && item.kind == nia_build::PlanArtifactKind::ObjectSet
                        && item.output.protocol_path() == "objects-dir"
                        && item.root_module.name() == "main"
                        && artifact == &item.key
        })
    ));
    let host_object_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "host-object-emit")
        .expect("host object emit action");
    assert!(matches!(
        &host_object_emit.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if target == plan.host_target()
                && plan.artifacts().iter().any(|item| {
                    item.key.name() == "host-objects"
                        && item.kind == nia_build::PlanArtifactKind::ObjectSet
                        && item.output.protocol_path() == "host-objects-dir"
                        && item.root_module.name() == "main"
                        && artifact == &item.key
        })
    ));
    let archive_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "archive-emit")
        .expect("archive emit action");
    assert!(matches!(
        &archive_emit.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if target == plan.artifact_target()
                && plan.artifacts().iter().any(|item| {
                    item.key.name() == "archive"
                        && item.kind == nia_build::PlanArtifactKind::StaticArchive
                        && item.output.protocol_path() == "archive"
                        && item.root_module.name() == "main"
                        && artifact == &item.key
        })
    ));
    let host_archive_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "host-archive-emit")
        .expect("host archive emit action");
    assert!(matches!(
        &host_archive_emit.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if target == plan.host_target()
                && plan.artifacts().iter().any(|item| {
                    item.key.name() == "host-archive"
                        && item.kind == nia_build::PlanArtifactKind::StaticArchive
                        && item.output.protocol_path() == "libhost-archive.a"
                        && item.root_module.name() == "main"
                        && artifact == &item.key
        })
    ));
    let object_input = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "object-input")
        .expect("object input action");
    assert!(matches!(
        &object_input.kind,
        nia_build::ActionKind::ExternalCommand { inputs, .. }
            if inputs.iter().any(|input| matches!(
                input.root(),
                nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "objects"
            ) && input.components().is_empty())
    ));
    let archive_input = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "archive-input")
        .expect("archive input action");
    assert!(matches!(
        &archive_input.kind,
        nia_build::ActionKind::ExternalCommand { inputs, .. }
            if inputs.iter().any(|input| matches!(
                input.root(),
                nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "archive"
            ) && input.components().is_empty())
    ));
    let archive_input_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "archive-input")
        .expect("archive input step");
    assert_eq!(
        archive_input_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["archive-emit"]
    );
    let linked_emit = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "linked-emit")
        .expect("typed static archive executable emit action");
    assert!(matches!(
        &linked_emit.kind,
        nia_build::ActionKind::CompilerEmit { static_archives, .. }
            if static_archives.iter().map(nia_build::ArtifactKey::name).collect::<Vec<_>>()
                == ["archive"]
    ));
    let linked_emit_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "linked-emit")
        .expect("typed static archive executable emit step");
    assert_eq!(
        linked_emit_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["archive-emit"]
    );
    for name in ["host-check", "host-emit"] {
        let action = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == name)
            .unwrap_or_else(|| panic!("missing {name} action"));
        match &action.kind {
            nia_build::ActionKind::CompilerCheck { target, .. }
            | nia_build::ActionKind::CompilerEmit { target, .. } => {
                assert_eq!(target, plan.host_target());
                assert_ne!(target, plan.artifact_target());
            }
            other => panic!("expected host compiler action, found {other:?}"),
        }
    }
    let host_test = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "host-test")
        .expect("host executable test action");
    assert!(matches!(
        &host_test.kind,
        nia_build::ActionKind::ExternalCommand {
            program: nia_build::CommandProgram::Path(path),
            ..
        }
        | nia_build::ActionKind::TestExecutable {
            program: nia_build::CommandProgram::Path(path),
            ..
        } if matches!(
            path.root(),
            nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "host-app"
        )
    ));
    let host_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "host-app")
        .expect("host executable artifact");
    assert_eq!(host_artifact.output.protocol_path(), "host-tool");
}
