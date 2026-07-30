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
        }
        if self.failFree {
            self.failFree = false;
            return mem::Error::Invalid!;
        }
        !{}
    }
}

fn testTarget(text: &[char]) build::TargetView {
    let value = string::StringView::init(text);
    build::TargetView::init(value, value, value, value, value, value, 64u32)
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
        false,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err != build::Error::OutOfMemory {
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
        false,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (14 as process::ExitCode)!;
        },
        err! => if err != build::Error::OutOfMemory {
            return (15 as process::ExitCode)!;
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
        false,
        1usize,
    ) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit().exit().?;
            return (11 as process::ExitCode)!;
        },
        err! => {
            if err == build::Error::OutOfMemory {
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
        false,
        1usize,
    ).exit().?;
    let mut cleaned = false;
    defer if not cleaned {
        api.deinit().exit().?;
    };

    let shortName = "a";
    let shortPath = "b";
    let rootSource = "m";
    let imports = [
        build::ModuleImport::init(&shortName, fs::PathView::init(&shortPath)),
        build::ModuleImport::init(&shortName, fs::PathView::init(&shortPath)),
    ];
    let beforeModule = allocator.activeAllocations;
    allocator.failAfter(5usize);
    switch api.addModule(
        build::ModuleOptions::init(fs::PathView::init(&rootSource)).withImports(&imports[..]),
    ) {
        !handle => {
            _ = handle;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if err != build::Error::OutOfMemory {
                return (5 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeModule {
        return (6 as process::ExitCode)!;
    }

    allocator.disableFailure();
    let moduleHandle = api.addModule(build::ModuleOptions::init(emptyPath)).exit().?;
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
            if err != build::Error::OutOfMemory {
                return (8 as process::ExitCode)!;
            }
        },
    }
    if allocator.activeAllocations != beforeTarget {
        return (9 as process::ExitCode)!;
    }
    cleaned = true;
    api.deinit().exit().?;
    if allocator.activeAllocations != 0usize {
        return (10 as process::ExitCode)!;
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    checkInitRollback(init).?;
    checkTargetInitRollback(init, 7usize).?;
    checkTargetInitRollback(init, 13usize).?;
    checkCleanupFailureOverridesExit(init).?;
    checkRecordRollback(init).?;
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
            .status_timeout("run build ownership rollback fixture")
            .code(),
        Some(0)
    );
}
