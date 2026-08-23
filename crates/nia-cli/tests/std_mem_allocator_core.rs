// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_mem_core_allocator_and_layout_cases() {
    let root = temp_dir("emit_exe_std_mem_core_allocator_and_layout_cases");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

fn check_page_allocator_allocates() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    match mem::Layout::of[u8]() {
        !value => { layout = value; },
        error! => { return process::exit(5)!; },
    }
    match allocator.allocBytes(4096, layout.align()) {
        !block => { let mut ptr = block.ptr();
                ptr.* = 42;
                if ptr.* != 42 {
                    return process::exit(2)!;
                }
                match allocator.free(block) {
                    !ok => { _ = ok; },
                    error! => { return process::exit(3)!; },
                } },
        error! => { return process::exit(1)!; },
    }
    !()
}

fn check_page_allocator_overaligned_layouts() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    match mem::Layout::init(64, 8192) {
        !value => { layout = value; },
        error! => { return process::exit(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(layout) {
        !value => { block = value; },
        error! => { return process::exit(2)!; },
    }
    if block.ptr() as usize % 8192 != 0 {
        return process::exit(3)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 17;
    bytes[63] = 23;
    if bytes[0] != 17 or bytes[63] != 23 {
        return process::exit(4)!;
    }
    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::exit(5)!; },
    }
    !()
}

fn check_layout_rejects_invalid_alignment() process::ExitCode!() {
    match mem::Layout::init(16, 3) {
        !ok => { _ = ok;
                return process::exit(1)!; },
        err! => { if err as i32 != mem::Error::InvalidAlignment as i32 {
                    return process::exit(2)!;
                } },
    }
    !()
}

fn check_layout_rejects_array_size_overflow() process::ExitCode!() {
    match mem::Layout::array[i32](4611686018427387904) {
        !ok => { _ = ok;
                return process::exit(1)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                    return process::exit(2)!;
                } },
    }
    !()
}

fn check_allocator_can_allocate_typed_slices() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    match allocator.allocSlice[i32](4) {
        mut !allocation => { allocation.asMutSlice()[0] = 10;
                allocation.asMutSlice()[1] = 20;
                allocation.asMutSlice()[2] = 30;
                allocation.asMutSlice()[3] = 40;
                if allocation.len() != 4 {
                    return process::exit(2)!;
                }
                if allocation.asSlice()[0] + allocation.asSlice()[1]
                    + allocation.asSlice()[2] + allocation.asSlice()[3] != 100 {
                    return process::exit(3)!;
                }
                allocation.deinit(&mut allocator).exit().?; },
        error! => { return process::exit(1)!; },
    }
    !()
}

struct NonEmptyZeroAllocator {
    backing: mem::PageAllocator,
    freeCount: usize,
}

extend NonEmptyZeroAllocator {
    fn init() NonEmptyZeroAllocator {
        Self { backing: mem::PageAllocator::init(), freeCount: 0 }
    }
}

extend NonEmptyZeroAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if layout.isEmpty() {
            self.backing.alloc(mem::Layout::init(1, layout.align()).?)
        } else {
            self.backing.alloc(layout)
        }
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if not block.isEmpty() {
            self.freeCount += 1;
        }
        self.backing.free(block)
    }
}

fn check_zero_length_slice_releases_block_owner() process::ExitCode!() {
    let mut allocator = NonEmptyZeroAllocator::init();
    let mut allocation = allocator.allocSlice[u8](0).exit().?;
    allocation.deinit(&mut allocator).exit().?;
    if allocator.freeCount != 1 {
        return process::exit(1)!;
    }
    allocation.deinit(&mut allocator).exit().?;
    if allocator.freeCount != 1 {
        return process::exit(2)!;
    }
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    check_page_allocator_allocates().?;
    check_page_allocator_overaligned_layouts().?;
    check_layout_rejects_invalid_alignment().?;
    check_layout_rejects_array_size_overflow().?;
    check_allocator_can_allocate_typed_slices().?;
    check_zero_length_slice_releases_block_owner().?;
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_fixed_buffer_allocator_resize_and_reset() {
    let root = temp_dir("emit_exe_std_mem_fixed_buffer_allocator_resize_and_reset");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 64] = [0; 64];
    let mut allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    let first_layout = mem::Layout::init(8, 8).exit().?;
    let mut first = allocator.alloc(first_layout).exit().?;
    if first.ptr() as usize % 8 != 0 {
        return process::exit(1)!;
    }

    let second_layout = mem::Layout::init(8, 1).exit().?;
    let mut second = allocator.alloc(second_layout).exit().?;
    if allocator.resize(first, mem::Layout::init(16, 8).exit().?) {
        return process::exit(2)!;
    }
    if not allocator.resize(first, mem::Layout::init(4, 8).exit().?) {
        return process::exit(3)!;
    }

    match allocator.remap(second, mem::Layout::init(24, 1).exit().?) {
        ?grown => { second = grown; },
        null => { return process::exit(4)!; },
    }
    if second.size() != 24 {
        return process::exit(5)!;
    }

    allocator.free(second).exit().?;
    match allocator.allocBytes(48, 1) {
        !block => { allocator.free(block).exit().?; },
        error! => { return process::exit(6)!; },
    }

    allocator.reset();
    if allocator.used() != 0 or allocator.remaining() != 64 {
        return process::exit(7)!;
    }

    match allocator.allocBytes(64, 1) {
        !block => { allocator.free(block).exit().?; },
        error! => { return process::exit(8)!; },
    }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocated_release_failure_can_retry() {
    let root = temp_dir("emit_exe_std_mem_allocated_release_failure_can_retry");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct FailOnceAllocator {
    backing: mem::PageAllocator,
    failNextFree: bool,
}

extend FailOnceAllocator {
    fn init() FailOnceAllocator {
        Self { backing: mem::PageAllocator::init(), failNextFree: false }
    }

    fn failNext(&mut self) () {
        self.failNextFree = true;
    }
}

extend FailOnceAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if self.failNextFree and not block.isEmpty() {
            self.failNextFree = false;
            return mem::Error::Invalid!;
        }
        self.backing.free(block)
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = FailOnceAllocator::init();
    let mut owned = mem::allocValue[u64](&mut allocator, 42u64).exit().?;
    allocator.failNext();
    match owned.deinit(&mut allocator) {
        !ok => {
            _ = ok;
            return process::exit(1)!;
        },
        mem::Error::Invalid! => {},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }
    owned.deinit(&mut allocator).exit().?;

    let base: i32 = 1;
    let mut callableStorage = mem::allocValue(
        &mut allocator,
        \[base] value: i32 -> { base + value },
    ).exit().?;
    let mut callableState = callableStorage.valueMut();
    let callableView: &mut Fn(i32) i32 = &mut callableState.*;
    let mut callableOwner = callableStorage.intoCallable(callableView);
    allocator.failNext();
    match callableOwner.deinit(&mut allocator) {
        !ok => {
            _ = ok;
            return process::exit(3)!;
        },
        mem::Error::Invalid! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }
    callableOwner.deinit(&mut allocator).exit().?;
    !()
}
"#,
    )
    .expect("write emitted allocator retry source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe allocator retry");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted allocator retry executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn std_mem_obsolete_allocator_spellings_are_absent() {
    let root = temp_dir("std_mem_obsolete_allocator_spellings_are_absent");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::mem;

fn probe(
    allocator: &mut mem::Allocator,
    layout: mem::Layout,
    block: mem::Block,
    arena: &mut mem::ArenaAllocator,
    fixed: &mem::FixedBufferAllocator,
) () {
    let mut bytes: [u8; 1] = [0];
    _ = allocator.alloc_bytes(1, 1);
    _ = allocator.alloc_slice[u8](1);
    _ = allocator.free_slice[u8](&mut bytes[..]);
    _ = layout.is_empty();
    _ = block.as_slice[u8]();
    _ = arena.query_capacity();
    _ = arena.query_used();
    _ = arena.reset_retain_capacity();
    _ = arena.reset_retain_with_limit(0);
    _ = fixed.owns_block(block);
    _ = fixed.is_last_allocation(block);
    _ = fixed.ownsBlock(block);
    _ = fixed.isLastAllocation(block);
}

fn main() () {}
"#,
    )
    .expect("write obsolete allocator API source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("check obsolete allocator API spellings");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for name in [
        "alloc_bytes",
        "alloc_slice",
        "free_slice",
        "is_empty",
        "as_slice",
        "query_capacity",
        "query_used",
        "reset_retain_capacity",
        "reset_retain_with_limit",
        "owns_block",
        "is_last_allocation",
        "ownsBlock",
        "isLastAllocation",
    ] {
        assert!(
            stderr.contains(name),
            "missing diagnostic for {name}:\n{stderr}"
        );
    }
}
