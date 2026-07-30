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

fn check_page_allocator_allocates() process::ExitCode!void {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    switch mem::Layout::of[u8]() {
        !value => { layout = value; },
        error! => { return (5 as process::ExitCode)!; },
    }
    switch allocator.alloc_bytes(4096, layout.align()) {
        !block => { let mut ptr = block.ptr();
                ptr.* = 42u8;
                if ptr.* != 42u8 {
                    return (2 as process::ExitCode)!;
                }
                switch allocator.free(block) {
                    !ok => { _ = ok; },
                    error! => { return (3 as process::ExitCode)!; },
                } },
        error! => { return (1 as process::ExitCode)!; },
    }
    !{}
}

fn check_page_allocator_overaligned_layouts() process::ExitCode!void {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    switch mem::Layout::init(64, 8192) {
        !value => { layout = value; },
        error! => { return (1 as process::ExitCode)!; },
    }
    let mut block: mem::Block;
    switch allocator.alloc(layout) {
        !value => { block = value; },
        error! => { return (2 as process::ExitCode)!; },
    }
    if block.ptr() as usize % 8192usize != 0usize {
        return (3 as process::ExitCode)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 17u8;
    bytes[63] = 23u8;
    if bytes[0] != 17u8 or bytes[63] != 23u8 {
        return (4 as process::ExitCode)!;
    }
    switch allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return (5 as process::ExitCode)!; },
    }
    !{}
}

fn check_layout_rejects_invalid_alignment() process::ExitCode!void {
    switch mem::Layout::init(16, 3) {
        !ok => { _ = ok;
                return (1 as process::ExitCode)!; },
        err! => { if err as i32 != mem::Error::InvalidAlignment as i32 {
                    return (2 as process::ExitCode)!;
                } },
    }
    !{}
}

fn check_layout_rejects_array_size_overflow() process::ExitCode!void {
    switch mem::Layout::array[i32](4611686018427387904usize) {
        !ok => { _ = ok;
                return (1 as process::ExitCode)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                    return (2 as process::ExitCode)!;
                } },
    }
    !{}
}

fn check_allocator_can_allocate_typed_slices() process::ExitCode!void {
    let mut allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[i32](4) {
        mut !items => { items[0] = 10;
                items[1] = 20;
                items[2] = 30;
                items[3] = 40;
                if items.len() != 4 {
                    return (2 as process::ExitCode)!;
                }
                if items[0] + items[1] + items[2] + items[3] != 100 {
                    return (3 as process::ExitCode)!;
                }
                switch allocator.free_slice[i32](items) {
                    !ok => { _ = ok; },
                    error! => { return (4 as process::ExitCode)!; },
                } },
        error! => { return (1 as process::ExitCode)!; },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    check_page_allocator_allocates().?;
    check_page_allocator_overaligned_layouts().?;
    check_layout_rejects_invalid_alignment().?;
    check_layout_rejects_array_size_overflow().?;
    check_allocator_can_allocate_typed_slices().?;
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

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut storage: [64]u8 = [_]u8[0; 64];
    let mut allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    let first_layout = mem::Layout::init(8, 8).exit().?;
    let mut first = allocator.alloc(first_layout).exit().?;
    if first.ptr() as usize % 8usize != 0usize {
        return (1 as process::ExitCode)!;
    }

    let second_layout = mem::Layout::init(8, 1).exit().?;
    let mut second = allocator.alloc(second_layout).exit().?;
    if allocator.resize(first, mem::Layout::init(16, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }
    if not allocator.resize(first, mem::Layout::init(4, 8).exit().?) {
        return (3 as process::ExitCode)!;
    }

    switch allocator.remap(second, mem::Layout::init(24, 1).exit().?) {
        ?grown => { second = grown; },
        null => { return (4 as process::ExitCode)!; },
    }
    if second.size() != 24 {
        return (5 as process::ExitCode)!;
    }

    allocator.free(second).exit().?;
    switch allocator.alloc_bytes(48, 1) {
        !block => { allocator.free(block).exit().?; },
        error! => { return (6 as process::ExitCode)!; },
    }

    allocator.reset();
    if allocator.used() != 0usize or allocator.remaining() != 64usize {
        return (7 as process::ExitCode)!;
    }

    switch allocator.alloc_bytes(64, 1) {
        !block => { allocator.free(block).exit().?; },
        error! => { return (8 as process::ExitCode)!; },
    }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
