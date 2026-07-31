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
            return process::exit(11)!;
        },
        err! => {
            if not isPackageRootReleaseInvalid(err) {
                reportUnexpected(init, err).?;
                return process::exit(12)!;
            }
        },
    }
    if allocator.activeAllocations != 0usize {
        return process::exit(13)!;
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
    let beforeTarget = allocator.activeAllocations;
    let targetName = "app";
    let outputName = "output";
    allocator.failAfter(1usize);
    switch api.addExecutable(
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
    switch api.addRunExecutableStep(
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
    switch api.addGeneratedFileStep(
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
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return process::exit(10)!;
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
    switch api.writePlanDraft(fs::PathView::init(&"plan.draft")) {
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
