// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn build_owned_inputs_roll_back_partial_allocations() {
    let root = temp_dir("build_owned_inputs_roll_back_partial_allocations");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::build;
using std::fs;
using std::io;
using std::mem;
using std::process;
using std::string;

struct FaultAllocator {
    backing: mem::PageAllocator,
    allocationAttempts: usize,
    activeAllocations: usize,
    failAt: usize,
    retainedFreeFailures: usize,
}

extend FaultAllocator {
    fn init() FaultAllocator {
        Self {
            backing: mem::PageAllocator::init(),
            allocationAttempts: 0usize,
            activeAllocations: 0usize,
            failAt: 0usize,
            retainedFreeFailures: 0usize,
        }
    }

    fn failAfter(&mut self, successfulAllocations: usize) () {
        self.failAt = self.allocationAttempts + successfulAllocations + 1usize;
    }

    fn disableFailure(&mut self) () {
        self.failAt = 0usize;
    }

    fn failNextRetainedFree(&mut self) () {
        self.failNextRetainedFrees(1usize);
    }

    fn failNextRetainedFrees(&mut self, count: usize) () {
        self.retainedFreeFailures = count;
    }
}

extend FaultAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if not layout.isEmpty() {
            self.allocationAttempts += 1usize;
            if self.failAt == self.allocationAttempts {
                return mem::Error::OutOfMemory!;
            }
        }
        let block = self.backing.alloc(layout).?;
        if not block.isEmpty() {
            self.activeAllocations += 1usize;
        }
        !block
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if not block.isEmpty() and self.retainedFreeFailures != 0usize {
            self.retainedFreeFailures -= 1usize;
            return mem::Error::Invalid!;
        }
        self.backing.free(block).?;
        if not block.isEmpty() {
            if self.activeAllocations == 0usize {
                return mem::Error::Invalid!;
            }
            self.activeAllocations -= 1usize;
        }
        !()
    }
}

fn testTarget(text: &[char]) build::TargetView {
    build::TargetView::init(text, text, text, text, text, text, 64u32)
}

fn isBuildDirRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::BuildDir,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isTargetRetainOom(error: build::Error, host: bool) bool {
    if host {
        match error {
            build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: build::ErrorSubject::HostTarget,
                cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
            } => true,
            _ => false,
        }
    } else {
        match error {
            build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: build::ErrorSubject::ArtifactTarget,
                cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
            } => true,
            _ => false,
        }
    }
}

fn isPackageRootReleaseInvalid(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Release,
            subject: build::ErrorSubject::PackageRoot,
            cause: build::ErrorCause::Memory(mem::Error::Invalid),
        } => true,
        _ => false,
    }
}

fn isImportRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::ModuleImport(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isPackageRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Package(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isExecutableRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Executable(_),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isObjectRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Object(0usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isStaticArchiveRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::StaticArchive(0usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isGeneratedFileRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Step(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isRunRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Step(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isExternalCommandRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Step(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isDependencyRetainOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Dependencies,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isPlanValidationOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Dependencies,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isStepCycle(error: build::Error, step: usize, dependency: usize) bool {
    match error {
        build::Error::StepCycle {
            step: actualStep,
            dependency: actualDependency,
        } => (actualStep == step and actualDependency == dependency)
            or (actualStep == dependency and actualDependency == step),
        _ => false,
    }
}

fn isPlanEncodingOom(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Encode,
            subject: build::ErrorSubject::BuildPlan,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isStepReleaseInvalid(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Release,
            subject: build::ErrorSubject::Step(_),
            cause: build::ErrorCause::Memory(mem::Error::Invalid),
        } => true,
        _ => false,
    }
}

fn isModuleImportReleaseInvalid(error: build::Error) bool {
    match error {
        build::Error::Failure {
            operation: build::ErrorOperation::Release,
            subject: build::ErrorSubject::ModuleImport(_),
            cause: build::ErrorCause::Memory(mem::Error::Invalid),
        } => true,
        _ => false,
    }
}

fn reportUnexpected(init: process::Init, error: build::Error) process::ExitCode!() {
    let mut buffer: [u8; 512] = [0; 512];
    let mut stderr = io::FileWriter::stderr(&mut buffer[..]);
    stderr.print(&"unexpected build error: {}\n", &[&error]).exit().?;
    stderr.flush().exit().?;
    !()
}

fn checkInitRollback(init: process::Init) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(1usize);
    let pathText: [char; 64] = ['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    let mut initialization = build::Build::init(
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    match initialization.finish() {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return process::exit(1)!;
        },
        err! => {
            if not isBuildDirRetainOom(err) {
                return process::exit(2)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return process::exit(3)!;
    }
    !()
}

fn checkTargetInitRollback(init: process::Init, successfulAllocations: usize) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(successfulAllocations);
    let pathText: [char; 64] = ['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    let mut initialization = build::Build::init(
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    match initialization.finish() {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return process::exit(14)!;
        },
        err! => {
            let host = successfulAllocations == 7usize;
            if not isTargetRetainOom(err, host) {
                return process::exit(15)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return process::exit(16)!;
    }
    !()
}

fn checkInitCleanupRetry(init: process::Init) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(1usize);
    allocator.failNextRetainedFree();
    let pathText: [char; 64] = ['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    let mut initialization = build::Build::init(
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    match initialization.finish() {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return process::exit(11)!;
        },
        err! => {
            if not isPackageRootReleaseInvalid(err) {
                reportUnexpected(init, err).?;
                return process::exit(12)!;
            }
        },
    }
    if allocator.activeAllocations != 1usize {
        return process::exit(13)!;
    }
    match initialization.finish() {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return process::exit(55)!;
        },
        err! => if not isBuildDirRetainOom(err) {
            return process::exit(56)!;
        },
    }
    if allocator.activeAllocations != 0usize {
        return process::exit(57)!;
    }
    !()
}

fn checkRecordRollback(init: process::Init) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    let empty = "";
    let emptyPath = fs::PathView::init(&empty);
    let target = testTarget(&empty);
    let mut initialization = build::Build::init(
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    let mut api = initialization.finish().exit().?;
    let mut cleaned = false;
    defer if not cleaned {
        api.deinit().exit().?;
    };

    let packageOptions = build::PackageOptions::init(
        &"dependency",
        fs::PathView::init(&"dependency"),
    );
    let beforePackage = allocator.activeAllocations;
    allocator.failAfter(2usize);
    allocator.failNextRetainedFree();
    match api.addPackage(packageOptions) {
        !handle => {
            _ = handle;
            return process::exit(58)!;
        },
        err! => if not isPackageRetainOom(err) {
            return process::exit(59)!;
        },
    }
    if allocator.activeAllocations != beforePackage + 2usize {
        return process::exit(60)!;
    }
    allocator.disableFailure();
    _ = api.addPackage(packageOptions).exit().?;

    let shortName = "a";
    let secondName = "c";
    let shortPath = "b";
    let rootSource = "m";
    let imports = [
        build::ModuleImport::init(&secondName, fs::PathView::init(&shortPath)),
        build::ModuleImport::init(&shortName, fs::PathView::init(&shortPath)),
    ];
    let beforeModule = allocator.activeAllocations;
    allocator.failAfter(6usize);
    match api.addModule(
        build::ModuleOptions::init(&"root", fs::PathView::init(&rootSource)).withImports(&imports[..]),
    ) {
        !handle => {
            _ = handle;
            return process::exit(4)!;
        },
        err! => {
            if not isImportRetainOom(err) {
                return process::exit(5)!;
            }
        },
    }
    if allocator.activeAllocations != beforeModule {
        return process::exit(6)!;
    }

    allocator.disableFailure();
    let moduleHandle = api.addModule(build::ModuleOptions::init(&"root", emptyPath)).exit().?;
    let objectOptions = build::ObjectOptions::init(&"object", moduleHandle)
        .withOutputName(&"object-output");
    let beforeObject = allocator.activeAllocations;
    allocator.failAfter(2usize);
    allocator.failNextRetainedFree();
    match api.addObject(objectOptions) {
        !handle => {
            _ = handle;
            return process::exit(61)!;
        },
        err! => if not isObjectRetainOom(err) {
            return process::exit(62)!;
        },
    }
    if allocator.activeAllocations != beforeObject + 2usize {
        return process::exit(63)!;
    }
    allocator.disableFailure();
    _ = api.addObject(objectOptions).exit().?;
    let archiveOptions = build::StaticArchiveOptions::init(&"archive", moduleHandle)
        .withOutputName(&"archive-output");
    let beforeArchive = allocator.activeAllocations;
    allocator.failAfter(2usize);
    allocator.failNextRetainedFree();
    match api.addStaticArchive(archiveOptions) {
        !handle => {
            _ = handle;
            return process::exit(64)!;
        },
        err! => if not isStaticArchiveRetainOom(err) {
            return process::exit(65)!;
        },
    }
    if allocator.activeAllocations != beforeArchive + 2usize {
        return process::exit(66)!;
    }
    allocator.disableFailure();
    let archive = api.addStaticArchive(archiveOptions).exit().?;
    let linkedArchives = [archive];
    let linkedOptions = build::ExecutableOptions::init(&"linked", moduleHandle)
        .withOutputName(&"linked-output")
        .withStaticArchives(&linkedArchives[..]);
    let beforeLinked = allocator.activeAllocations;
    allocator.failAfter(3usize);
    allocator.failNextRetainedFrees(2usize);
    match api.addExecutable(linkedOptions) {
        !handle => {
            _ = handle;
            return process::exit(67)!;
        },
        err! => if not isExecutableRetainOom(err) {
            return process::exit(68)!;
        },
    }
    if allocator.activeAllocations != beforeLinked + 3usize {
        return process::exit(69)!;
    }
    allocator.disableFailure();
    _ = api.addExecutable(linkedOptions).exit().?;
    let beforeTarget = allocator.activeAllocations;
    let targetName = "app";
    let outputName = "output";
    allocator.failAfter(1usize);
    match api.addExecutable(
        build::ExecutableOptions::init(&targetName, moduleHandle).withOutputName(&outputName),
    ) {
        !handle => {
            _ = handle;
            return process::exit(7)!;
        },
        err! => {
            if not isExecutableRetainOom(err) {
                return process::exit(8)!;
            }
        },
    }
    if allocator.activeAllocations != beforeTarget {
        return process::exit(9)!;
    }
    allocator.disableFailure();
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&targetName, moduleHandle).withOutputName(&outputName),
    ).exit().?;
    _ = api.addEmitExecutableStep(&"emit", executable).exit().?;
    let runArguments: [&[char]; 2] = [&"first", &"second"];
    let beforeRun = allocator.activeAllocations;
    allocator.failAfter(1usize);
    match api.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ) {
        !handle => {
            _ = handle;
            return process::exit(30)!;
        },
        err! => {
            if not isRunRetainOom(err) {
                return process::exit(31)!;
            }
        },
    }
    if allocator.activeAllocations != beforeRun {
        return process::exit(32)!;
    }
    allocator.disableFailure();
    let beforeDependency = allocator.activeAllocations;
    allocator.failAfter(4usize);
    match api.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ) {
        !handle => {
            _ = handle;
            return process::exit(33)!;
        },
        err! => {
            if not isDependencyRetainOom(err) {
                return process::exit(34)!;
            }
        },
    }
    if allocator.activeAllocations != beforeDependency {
        return process::exit(35)!;
    }
    allocator.disableFailure();
    let beforeGenerated = allocator.activeAllocations;
    allocator.failAfter(1usize);
    match api.addGeneratedFileStep(
        &"generate",
        build::BuildPathView::init(&"generated/source.nia"),
        &b"contents"[..],
    ) {
        !handle => {
            _ = handle;
            return process::exit(27)!;
        },
        err! => {
            if not isGeneratedFileRetainOom(err) {
                return process::exit(28)!;
            }
        },
    }
    if allocator.activeAllocations != beforeGenerated {
        return process::exit(29)!;
    }
    allocator.disableFailure();
    let commandArguments = [
        build::CommandArgument::literal(&"first"),
        build::CommandArgument::packageInput(api.rootPackage(), fs::PathView::init(&"input.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"output.txt")),
    ];
    let beforeCommand = allocator.activeAllocations;
    allocator.failAfter(3usize);
    match api.addExternalCommandStep(
        &"tool",
        build::ExternalCommandOptions::search(&"tool").withArguments(&commandArguments),
    ) {
        !handle => {
            _ = handle;
            return process::exit(36)!;
        },
        err! => {
            if not isExternalCommandRetainOom(err) {
                return process::exit(37)!;
            }
        },
    }
    if allocator.activeAllocations != beforeCommand {
        return process::exit(38)!;
    }
    allocator.disableFailure();
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return process::exit(10)!;
    }
    !()
}

fn checkArgAssemblyRollback(init: process::Init) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    let empty = "";
    let emptyPath = fs::PathView::init(&empty);
    let target = testTarget(&empty);
    let mut initialization = build::Build::init(
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    let mut api = initialization.finish().exit().?;
    let mut cleaned = false;
    defer if not cleaned {
        api.deinit().exit().?;
    };
    let importName = "dependency";
    let importPath = "dependency.nia";
    let imports = [
        build::ModuleImport::init(&importName, fs::PathView::init(&importPath)),
    ];
    let moduleHandle = api.addModule(
        build::ModuleOptions::init(&"root", fs::PathView::init(&"main.nia"))
            .withImports(&imports[..]),
    ).exit().?;
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&"app", moduleHandle),
    ).exit().?;
    let emit = api.addEmitExecutableStep(&"emit", executable).exit().?;
    api.setDefaultStep(emit).exit().?;
    let beforeValidate = allocator.activeAllocations;
    allocator.failAfter(1usize);
    match api.validatePlan() {
        !ok => {
            _ = ok;
            return process::exit(21)!;
        },
        err! => if not isPlanValidationOom(err) {
            return process::exit(22)!;
        },
    }
    if allocator.activeAllocations != beforeValidate {
        return process::exit(23)!;
    }
    allocator.disableFailure();
    api.validatePlan().exit().?;
    let beforeEncode = allocator.activeAllocations;
    allocator.failAfter(0usize);
    match api.writePlanDraft(fs::PathView::init(&"plan.draft")) {
        !ok => {
            _ = ok;
            return process::exit(24)!;
        },
        err! => if not isPlanEncodingOom(err) {
            return process::exit(25)!;
        },
    }
    if allocator.activeAllocations != beforeEncode {
        return process::exit(26)!;
    }
    allocator.disableFailure();
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return process::exit(20)!;
    }
    !()
}

fn checkCleanupRetryRetainsNestedOwners(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = FaultAllocator::init();
    let emptyPath = fs::PathView::init(&"");
    let target = testTarget(&"");
    let mut initialization = build::Build::init(
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    let mut api = initialization.finish().exit().?;
    let moduleHandle = api.addModule(
        build::ModuleOptions::init(&"root", fs::PathView::init(&"main.nia")),
    ).exit().?;
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&"app", moduleHandle),
    ).exit().?;
    _ = api.addEmitExecutableStep(&"emit", executable).exit().?;
    let runArguments: [&[char]; 2] = [&"first", &"second"];
    _ = api.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ).exit().?;

    allocator.failNextRetainedFree();
    match api.deinit() {
        !ok => {
            _ = ok;
            return process::exit(40)!;
        },
        err! => if not isStepReleaseInvalid(err) {
            return process::exit(41)!;
        },
    }
    // The failed string owner plus both containing list allocations must stay
    // attached until a later cleanup attempt can reach them.
    if allocator.activeAllocations != 3usize {
        return process::exit(42)!;
    }
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return process::exit(43)!;
    }

    let mut importAllocator = FaultAllocator::init();
    let mut importInitialization = build::Build::init(
        &mut importAllocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    let mut importApi = importInitialization.finish().exit().?;
    let imports = [
        build::ModuleImport::init(&"dependency", fs::PathView::init(&"dependency.nia")),
    ];
    _ = importApi.addModule(
        build::ModuleOptions::init(&"root", fs::PathView::init(&"main.nia"))
            .withImports(&imports[..]),
    ).exit().?;
    importAllocator.failNextRetainedFree();
    match importApi.deinit() {
        !ok => {
            _ = ok;
            return process::exit(44)!;
        },
        err! => if not isModuleImportReleaseInvalid(err) {
            return process::exit(45)!;
        },
    }
    if importAllocator.activeAllocations != 3usize {
        return process::exit(46)!;
    }
    importApi.deinit().exit().?;
    if importAllocator.activeAllocations != 0usize {
        return process::exit(47)!;
    }
    !()
}

fn checkValidationScratchCleanupRetry(init: process::Init) process::ExitCode!() {
    let mut allocator = FaultAllocator::init();
    let emptyPath = fs::PathView::init(&"");
    let target = testTarget(&"");
    let mut initialization = build::Build::init(
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        build::OptimizationMode::O0,
        1u32,
        null,
    );
    let mut api = initialization.finish().exit().?;
    let first = api.addAggregateStep(&"first").exit().?;
    let second = api.addAggregateStep(&"second").exit().?;
    api.setDefaultStep(first).exit().?;
    api.dependOn(first, second).exit().?;
    api.dependOn(second, first).exit().?;
    let beforeValidate = allocator.activeAllocations;

    allocator.failNextRetainedFrees(2usize);
    match api.validatePlan() {
        !ok => {
            _ = ok;
            return process::exit(48)!;
        },
        err! => if not isStepCycle(err, 0usize, 1usize) {
            reportUnexpected(init, err).?;
            return process::exit(49)!;
        },
    }
    if allocator.activeAllocations != beforeValidate + 2usize {
        return process::exit(50)!;
    }

    match api.validatePlan() {
        !ok => {
            _ = ok;
            return process::exit(51)!;
        },
        err! => if not isStepCycle(err, 0usize, 1usize) {
            return process::exit(52)!;
        },
    }
    if allocator.activeAllocations != beforeValidate {
        return process::exit(53)!;
    }
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return process::exit(54)!;
    }
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    checkInitRollback(init).?;
    checkTargetInitRollback(init, 7usize).?;
    checkTargetInitRollback(init, 13usize).?;
    checkInitCleanupRetry(init).?;
    checkRecordRollback(init).?;
    checkArgAssemblyRollback(init).?;
    checkCleanupRetryRetainsNestedOwners(init).?;
    checkValidationScratchCleanupRetry(init).?;
    !()
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
        .output_timeout_for_build("compile build ownership rollback fixture");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run build ownership rollback fixture")
            .code(),
        Some(0)
    );
    assert!(!root.join("plan.draft").exists());
}
