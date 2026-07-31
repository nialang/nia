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
    failFree: bool,
}

extend FaultAllocator {
    fn init() FaultAllocator {
        {
            backing: mem::PageAllocator::init(),
            allocationAttempts: 0usize,
            activeAllocations: 0usize,
            failAt: 0usize,
            failFree: false,
        }
    }

    fn failAfter(&mut self, successfulAllocations: usize) void {
        self.failAt = self.allocationAttempts + successfulAllocations + 1usize;
    }

    fn disableFailure(&mut self) void {
        self.failAt = 0usize;
    }

    fn failNextFree(&mut self) void {
        self.failFree = true;
    }
}

extend FaultAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if not layout.is_empty() {
            self.allocationAttempts += 1usize;
            if self.failAt == self.allocationAttempts {
                return mem::Error::OutOfMemory!;
            }
        }
        let block = self.backing.alloc(layout).?;
        if not block.is_empty() {
            self.activeAllocations += 1usize;
        }
        !block
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        self.backing.free(block).?;
        if not block.is_empty() {
            if self.activeAllocations == 0usize {
                return mem::Error::Invalid!;
            }
            self.activeAllocations -= 1usize;
            if self.failFree {
                self.failFree = false;
                return mem::Error::Invalid!;
            }
        }
        !{}
    }
}

fn testTarget(text: &[char]) build::TargetView {
    let value = string::StringView::init(text);
    build::TargetView::init(value, value, value, value, value, value, 64u32)
}

fn isBuildDirRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::BuildDir,
            cause: build::ErrorCause::FileSystem(fs::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isTargetRetainOom(error: build::Error, host: bool) bool {
    if host {
        switch error {
            build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: build::ErrorSubject::HostTarget,
                cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
            } => true,
            _ => false,
        }
    } else {
        switch error {
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
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Release,
            subject: build::ErrorSubject::PackageRoot,
            cause: build::ErrorCause::FileSystem(fs::Error::Invalid),
        } => true,
        _ => false,
    }
}

fn isImportRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::ModuleImport(1usize),
            cause: build::ErrorCause::FileSystem(fs::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isExecutableRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Executable(0usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isGeneratedFileRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Step(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isRunRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Step(1usize),
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isDependencyRetainOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Dependencies,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isPlanValidationOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Retain,
            subject: build::ErrorSubject::Dependencies,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn isPlanEncodingOom(error: build::Error) bool {
    switch error {
        build::Error::Failure {
            operation: build::ErrorOperation::Encode,
            subject: build::ErrorSubject::BuildPlan,
            cause: build::ErrorCause::Memory(mem::Error::OutOfMemory),
        } => true,
        _ => false,
    }
}

fn reportUnexpected(init: process::Init, error: build::Error) process::ExitCode!void {
    let mut buffer: [512]u8 = [_]u8[0; 512];
    let mut stderr = io::FileWriter::stderr(init.io(), &mut buffer[..]);
    stderr.print(&"unexpected build error: {}\n", &[&error]).exit().?;
    stderr.flush().exit().?;
    !{}
}

fn checkInitRollback(init: process::Init) process::ExitCode!void {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(1usize);
    let pathText: [64]char = [_]char['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    switch build::Build::init(
        init,
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        1u32,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if not isBuildDirRetainOom(err) {
                return (2 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return (3 as process::ExitCode)!;
    }
    !{}
}

fn checkTargetInitRollback(init: process::Init, successfulAllocations: usize) process::ExitCode!void {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(successfulAllocations);
    let pathText: [64]char = [_]char['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    switch build::Build::init(
        init,
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        1u32,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (14 as process::ExitCode)!;
        },
        err! => {
            let host = successfulAllocations == 7usize;
            if not isTargetRetainOom(err, host) {
                return (15 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return (16 as process::ExitCode)!;
    }
    !{}
}

fn checkCleanupFailureOverridesExit(init: process::Init) process::ExitCode!void {
    let mut allocator = FaultAllocator::init();
    allocator.failAfter(1usize);
    allocator.failNextFree();
    let pathText: [64]char = [_]char['p'; 64];
    let path = fs::PathView::init(&pathText);
    let target = testTarget(&pathText);
    switch build::Build::init(
        init,
        &mut allocator,
        path,
        path,
        path,
        path,
        path,
        target,
        target,
        1u32,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (11 as process::ExitCode)!;
        },
        err! => {
            if not isPackageRootReleaseInvalid(err) {
                reportUnexpected(init, err).?;
                return (12 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return (13 as process::ExitCode)!;
    }
    !{}
}

fn checkRecordRollback(init: process::Init) process::ExitCode!void {
    let mut allocator = FaultAllocator::init();
    let empty = "";
    let emptyPath = fs::PathView::init(&empty);
    let target = testTarget(&empty);
    let mut api = build::Build::init(
        init,
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        1u32,
        1usize,
    ).exit().?;
    let mut cleaned = false;
    defer if not cleaned {
        api.deinit().exit().?;
    };

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
    switch api.addModule(
        build::ModuleOptions::init(&"root", fs::PathView::init(&rootSource)).withImports(&imports[..]),
    ) {
        !handle => {
            _ = handle;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if not isImportRetainOom(err) {
                return (5 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeModule {
        return (6 as process::ExitCode)!;
    }

    allocator.disableFailure();
    let moduleHandle = api.addModule(build::ModuleOptions::init(&"root", emptyPath)).exit().?;
    let beforeTarget = allocator.activeAllocations;
    let targetName = "app";
    let outputName = "output";
    allocator.failAfter(1usize);
    switch api.addExecutable(
        build::ExecutableOptions::init(&targetName, moduleHandle).withOutputName(&outputName),
    ) {
        !handle => {
            _ = handle;
            return (7 as process::ExitCode)!;
        },
        err! => {
            if not isExecutableRetainOom(err) {
                return (8 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeTarget {
        return (9 as process::ExitCode)!;
    }
    allocator.disableFailure();
    let executable = api.addExecutable(
        build::ExecutableOptions::init(&targetName, moduleHandle).withOutputName(&outputName),
    ).exit().?;
    _ = api.addEmitExecutableStep(&"emit", executable).exit().?;
    let runArguments = [
        string::StringView::init(&"first"),
        string::StringView::init(&"second"),
    ];
    let beforeRun = allocator.activeAllocations;
    allocator.failAfter(1usize);
    switch api.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ) {
        !handle => {
            _ = handle;
            return (30 as process::ExitCode)!;
        },
        err! => {
            if not isRunRetainOom(err) {
                return (31 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeRun {
        return (32 as process::ExitCode)!;
    }
    allocator.disableFailure();
    let beforeDependency = allocator.activeAllocations;
    allocator.failAfter(4usize);
    switch api.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ) {
        !handle => {
            _ = handle;
            return (33 as process::ExitCode)!;
        },
        err! => {
            if not isDependencyRetainOom(err) {
                return (34 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeDependency {
        return (35 as process::ExitCode)!;
    }
    allocator.disableFailure();
    let beforeGenerated = allocator.activeAllocations;
    allocator.failAfter(1usize);
    switch api.addGeneratedFileStep(
        &"generate",
        build::BuildPathView::init(&"generated/source.nia"),
        &b"contents"[..],
    ) {
        !handle => {
            _ = handle;
            return (27 as process::ExitCode)!;
        },
        err! => {
            if not isGeneratedFileRetainOom(err) {
                return (28 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeGenerated {
        return (29 as process::ExitCode)!;
    }
    allocator.disableFailure();
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return (10 as process::ExitCode)!;
    }
    !{}
}

fn checkArgAssemblyRollback(init: process::Init) process::ExitCode!void {
    let mut allocator = FaultAllocator::init();
    let empty = "";
    let emptyPath = fs::PathView::init(&empty);
    let target = testTarget(&empty);
    let mut api = build::Build::init(
        init,
        &mut allocator,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        emptyPath,
        target,
        target,
        1u32,
        1usize,
    ).exit().?;
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
    switch api.validatePlan() {
        !ok => {
            _ = ok;
            return (21 as process::ExitCode)!;
        },
        err! => if not isPlanValidationOom(err) {
            return (22 as process::ExitCode)!;
        },
    }
    if allocator.activeAllocations != beforeValidate {
        return (23 as process::ExitCode)!;
    }
    allocator.disableFailure();
    api.validatePlan().exit().?;
    let beforeEncode = allocator.activeAllocations;
    allocator.failAfter(0usize);
    switch api.writePlanDraft(fs::PathView::init(&"plan.draft")) {
        !ok => {
            _ = ok;
            return (24 as process::ExitCode)!;
        },
        err! => if not isPlanEncodingOom(err) {
            return (25 as process::ExitCode)!;
        },
    }
    if allocator.activeAllocations != beforeEncode {
        return (26 as process::ExitCode)!;
    }
    allocator.disableFailure();
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return (20 as process::ExitCode)!;
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    checkInitRollback(init).?;
    checkTargetInitRollback(init, 7usize).?;
    checkTargetInitRollback(init, 13usize).?;
    checkCleanupFailureOverridesExit(init).?;
    checkRecordRollback(init).?;
    checkArgAssemblyRollback(init).?;
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
