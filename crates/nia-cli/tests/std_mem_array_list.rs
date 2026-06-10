// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{temp_dir, CommandExt};

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
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::of[u8]() {
        !value => layout = value,
        error! => return (5 as process::ExitCode)!,
    }
    switch allocator.alloc_bytes(4096, layout.align()) {
        !block => {
            var ptr = block.ptr();
            ptr.* = 42u8;
            if ptr.* != 42u8 {
                return (2 as process::ExitCode)!;
            }
            switch allocator.free(block) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}

fn check_page_allocator_overaligned_layouts() process::ExitCode!void {
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::init(64, 8192) {
        !value => layout = value,
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if block.ptr() as usize % 8192usize != 0usize {
        return (3 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 17u8;
    bytes[63] = 23u8;
    if bytes[0] != 17u8 or bytes[63] != 23u8 {
        return (4 as process::ExitCode)!;
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    !{}
}

fn check_layout_rejects_invalid_alignment() process::ExitCode!void {
    switch mem::Layout::init(16, 3) {
        !ok => {
            _ = ok;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::InvalidAlignment as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }
    !{}
}

fn check_layout_rejects_array_size_overflow() process::ExitCode!void {
    switch mem::Layout::array[i32](4611686018427387904usize) {
        !ok => {
            _ = ok;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }
    !{}
}

fn check_allocator_can_allocate_typed_slices() process::ExitCode!void {
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[i32](4) {
        !items => {
            items[0] = 10;
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
                !ok => _ = ok,
                error! => return (4 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_fixed_buffer_allocator_supports_array_list() {
    let root = temp_dir("emit_exe_std_mem_fixed_buffer_allocator_supports_array_list");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [256]u8 = [_]u8[0; 256];
    var allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    var list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }
    if allocator.used() == 0usize {
        return (2 as process::ExitCode)!;
    }

    list.deinit(&mut allocator).exit().?;
    allocator.reset();
    if allocator.used() != 0usize or allocator.remaining() != allocator.capacity() {
        return (3 as process::ExitCode)!;
    }

    var tiny: [8]u8 = [_]u8[0; 8];
    var failing = mem::FixedBufferAllocator::init(&mut tiny[..]);
    switch failing.alloc_bytes(16, 1) {
        !block => {
            _ = block;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return (5 as process::ExitCode)!;
            }
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var storage: [64]u8 = [_]u8[0; 64];
    var allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    let first_layout = mem::Layout::init(8, 8).exit().?;
    var first = allocator.alloc(first_layout).exit().?;
    if first.ptr() as usize % 8usize != 0usize {
        return (1 as process::ExitCode)!;
    }

    let second_layout = mem::Layout::init(8, 1).exit().?;
    var second = allocator.alloc(second_layout).exit().?;
    if allocator.resize(first, mem::Layout::init(16, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }
    if not allocator.resize(first, mem::Layout::init(4, 8).exit().?) {
        return (3 as process::ExitCode)!;
    }

    switch allocator.remap(second, mem::Layout::init(24, 1).exit().?) {
        ?grown => second = grown,
        null => return (4 as process::ExitCode)!,
    }
    if second.size() != 24 {
        return (5 as process::ExitCode)!;
    }

    allocator.free(second).exit().?;
    switch allocator.alloc_bytes(48, 1) {
        !block => {
            allocator.free(block).exit().?;
        },
        error! => return (6 as process::ExitCode)!,
    }

    allocator.reset();
    if allocator.used() != 0usize or allocator.remaining() != 64usize {
        return (7 as process::ExitCode)!;
    }

    switch allocator.alloc_bytes(64, 1) {
        !block => {
            allocator.free(block).exit().?;
        },
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_arena_allocator_supports_array_list_and_retain_reset() {
    let root = temp_dir("emit_exe_std_mem_arena_allocator_supports_array_list_and_retain_reset");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    var list = std::ArrayList[i32]::init();
    list.push(&mut arena, 10).exit().?;
    list.push(&mut arena, 20).exit().?;
    list.push(&mut arena, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }

    let capacity = arena.query_capacity();
    if capacity == 0usize or arena.query_used() == 0usize {
        return (2 as process::ExitCode)!;
    }

    arena.reset_retain_capacity().exit().?;
    if arena.query_capacity() != capacity or arena.query_used() != 0usize {
        return (3 as process::ExitCode)!;
    }

    var bytes = arena.alloc_slice[u8](64).exit().?;
    bytes[0] = 7u8;
    bytes[63] = 9u8;
    if bytes[0] != 7u8 or bytes[63] != 9u8 {
        return (4 as process::ExitCode)!;
    }

    arena.reset_retain_with_limit(0).exit().?;
    if arena.query_capacity() != 0usize or arena.query_used() != 0usize {
        return (5 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges() {
    let root = temp_dir("emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    var first = arena.alloc_bytes(16, 8).exit().?;
    var second = arena.alloc_bytes(16, 8).exit().?;
    if arena.resize(first, mem::Layout::init(32, 8).exit().?) {
        return (1 as process::ExitCode)!;
    }
    if not arena.resize(first, mem::Layout::init(8, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }

    switch arena.remap(second, mem::Layout::init(40, 8).exit().?) {
        ?grown => second = grown,
        null => return (3 as process::ExitCode)!,
    }
    if second.size() != 40 {
        return (4 as process::ExitCode)!;
    }

    arena.free(second).exit().?;
    switch arena.alloc_bytes(40, 8) {
        !again => {
            if again.ptr() as usize != second.ptr() as usize {
                return (5 as process::ExitCode)!;
            }
        },
        error! => return (6 as process::ExitCode)!,
    }

    arena.reset().exit().?;
    if arena.query_capacity() != 0usize or arena.query_used() != 0usize {
        return (7 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_supports_small_allocations_and_array_list() {
    let root = temp_dir(
        "emit_exe_std_mem_general_purpose_allocator_supports_small_allocations_and_array_list",
    );
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(24, 8).exit().?;
    var first = allocator.alloc(layout).exit().?;
    var second = allocator.alloc(layout).exit().?;
    if first.ptr() as usize == second.ptr() as usize {
        return (1 as process::ExitCode)!;
    }

    let first_addr = first.ptr() as usize;
    allocator.free(first).exit().?;
    var reused = allocator.alloc(layout).exit().?;
    if reused.ptr() as usize != first_addr {
        return (2 as process::ExitCode)!;
    }

    allocator.free(reused).exit().?;
    allocator.free(second).exit().?;
    if not allocator.is_empty() {
        return (3 as process::ExitCode)!;
    }

    var list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (4 as process::ExitCode)!;
    }
    if allocator.query_used() == 0usize or allocator.query_capacity() == 0usize {
        return (5 as process::ExitCode)!;
    }
    list.deinit(&mut allocator).exit().?;
    allocator.deinit().exit().?.ok().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_supports_large_overaligned_realloc() {
    let root =
        temp_dir("emit_exe_std_mem_general_purpose_allocator_supports_large_overaligned_realloc");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(3000, 4096).exit().?;
    var block = allocator.alloc(layout).exit().?;
    if block.ptr() as usize % 4096usize != 0usize {
        return (1 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 11u8;
    bytes[2999] = 22u8;

    let grown_layout = mem::Layout::init(3040, 4096).exit().?;
    let old_addr = block.ptr() as usize;
    block = allocator.realloc(block, grown_layout).exit().?;
    if block.ptr() as usize != old_addr or block.size() != 3040usize {
        return (2 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11u8 or bytes[2999] != 22u8 {
        return (3 as process::ExitCode)!;
    }

    let moved_layout = mem::Layout::init(12000, 4096).exit().?;
    block = allocator.realloc(block, moved_layout).exit().?;
    if block.ptr() as usize % 4096usize != 0usize or block.size() != 12000usize {
        return (4 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11u8 or bytes[2999] != 22u8 {
        return (5 as process::ExitCode)!;
    }

    allocator.free(block).exit().?;
    if not allocator.is_empty() {
        return (6 as process::ExitCode)!;
    }
    allocator.deinit().exit().?.ok().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_rejects_invalid_free_and_resize() {
    let root =
        temp_dir("emit_exe_std_mem_general_purpose_allocator_rejects_invalid_free_and_resize");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let small_layout = mem::Layout::init(32, 8).exit().?;
    let small = allocator.alloc(small_layout).exit().?;
    allocator.free(small).exit().?;
    switch allocator.free(small) {
        !ok => {
            _ = ok;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }
    if allocator.resize(small, small_layout) {
        return (3 as process::ExitCode)!;
    }

    var resized = allocator.alloc(small_layout).exit().?;
    let resized_layout = mem::Layout::init(40, 8).exit().?;
    if not allocator.resize(resized, resized_layout) {
        return (7 as process::ExitCode)!;
    }
    if allocator.query_used() != 40usize {
        return (8 as process::ExitCode)!;
    }
    switch allocator.free(resized) {
        !ok => {
            _ = ok;
            return (9 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (10 as process::ExitCode)!;
            }
        },
    }
    resized = mem::Block::init(resized.ptr(), resized_layout);
    allocator.free(resized).exit().?;

    let align_layout = mem::Layout::init(1, 1).exit().?;
    let align_block = allocator.alloc(align_layout).exit().?;
    if allocator.resize(align_block, mem::Layout::init(1, 2).exit().?) {
        return (11 as process::ExitCode)!;
    }
    allocator.free(align_block).exit().?;

    let large_layout = mem::Layout::init(4096, 4096).exit().?;
    let large = allocator.alloc(large_layout).exit().?;
    let wrong_layout = mem::Layout::init(2048, 4096).exit().?;
    let wrong = mem::Block::init(large.ptr(), wrong_layout);
    switch allocator.free(wrong) {
        !ok => {
            _ = ok;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (5 as process::ExitCode)!;
            }
        },
    }
    allocator.free(large).exit().?;
    if not allocator.is_empty() {
        return (6 as process::ExitCode)!;
    }
    allocator.deinit().exit().?.ok().exit().?;
    var leaking = mem::GeneralPurposeAllocator::init(&mut page);
    _ = leaking.alloc(small_layout).exit().?;
    if leaking.deinit().exit().? != mem::DeinitStatus::Leak {
        return (13 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_preserves_byte_prefix() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_preserves_byte_prefix");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var block: mem::Block;
    switch allocator.alloc_bytes(4, 1) {
        !value => block = value,
        error! => return (1 as process::ExitCode)!,
    }
    var bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;
    bytes[2] = 30u8;
    bytes[3] = 40u8;

    var grow_layout: mem::Layout;
    switch mem::Layout::init(8, 1) {
        !value => grow_layout = value,
        error! => return (2 as process::ExitCode)!,
    }
    switch allocator.realloc(block, grow_layout) {
        !value => block = value,
        error! => return (3 as process::ExitCode)!,
    }
    if block.size() != 8 {
        return (4 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 or bytes[2] != 30u8 or bytes[3] != 40u8 {
        return (5 as process::ExitCode)!;
    }
    bytes[4] = 50u8;
    bytes[5] = 60u8;

    var shrink_layout: mem::Layout;
    switch mem::Layout::init(2, 1) {
        !value => shrink_layout = value,
        error! => return (6 as process::ExitCode)!,
    }
    switch allocator.realloc(block, shrink_layout) {
        !value => block = value,
        error! => return (7 as process::ExitCode)!,
    }
    if block.size() != 2 {
        return (8 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 {
        return (9 as process::ExitCode)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct CountingAllocator {
    buffer: &mut [u8],
    end_index: usize,
    free_count: usize,
}

extend CountingAllocator {
    fn init(buffer: &mut [u8]) CountingAllocator {
        { buffer: buffer, end_index: 0, free_count: 0 }
    }
}

extend CountingAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if layout.is_empty() {
            return !mem::Block::init(layout.align() as &mut u8, layout);
        }
        let base = self.buffer.get_ptr() as usize;
        let current = base + self.end_index;
        let aligned = switch current.align_forward(layout.align()) {
            ?value => value,
            null => return mem::Error::OutOfMemory!,
        };
        let offset = aligned - base;
        let next = switch offset.checked_add(layout.size()) {
            ?value => value,
            null => return mem::Error::OutOfMemory!,
        };
        if next > self.buffer.len() {
            return mem::Error::OutOfMemory!;
        }
        self.end_index = next;
        !mem::Block::init(aligned as &mut u8, layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        if block.is_empty() {
            return !{};
        }
        self.free_count += 1usize;
        if self.free_count == 1usize {
            return mem::Error::Invalid!;
        }
        !{}
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [64]u8 = [_]u8[0; 64];
    var allocator = CountingAllocator::init(storage);
    let old_layout = mem::Layout::init(4, 1).exit().?;
    let new_layout = mem::Layout::init(8, 1).exit().?;
    var block = allocator.alloc(old_layout).exit().?;
    var bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;

    switch allocator.realloc(block, new_layout) {
        !new_block => {
            _ = new_block;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }

    if allocator.free_count != 2usize {
        return (3 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics() {
    let root = temp_dir("emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::init(16, 8) {
        !value => layout = value,
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if not allocator.resize(block, layout) {
        return (3 as process::ExitCode)!;
    }

    var larger: mem::Layout;
    switch mem::Layout::init(32, 8) {
        !value => larger = value,
        error! => return (4 as process::ExitCode)!,
    }
    if not allocator.resize(block, larger) {
        return (5 as process::ExitCode)!;
    }
    switch allocator.remap(block, larger) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                return (6 as process::ExitCode)!;
            }
            block = same;
        },
        null => return (7 as process::ExitCode)!,
    }
    switch allocator.remap(block, layout) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                return (8 as process::ExitCode)!;
            }
            block = same;
        },
        null => return (9 as process::ExitCode)!,
    }

    var next_page: mem::Layout;
    switch mem::Layout::init(8192, 8) {
        !value => next_page = value,
        error! => return (10 as process::ExitCode)!,
    }
    if allocator.resize(block, next_page) {
        return (11 as process::ExitCode)!;
    }
    switch allocator.remap(block, next_page) {
        ?moved => {
            _ = moved;
            return (12 as process::ExitCode)!;
        },
        null => {},
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }

    var empty_a: mem::Layout;
    switch mem::Layout::init(0, 8) {
        !value => empty_a = value,
        error! => return (14 as process::ExitCode)!,
    }
    switch allocator.alloc(empty_a) {
        !value => block = value,
        error! => return (15 as process::ExitCode)!,
    }
    var empty_b: mem::Layout;
    switch mem::Layout::init(0, 16) {
        !value => empty_b = value,
        error! => return (16 as process::ExitCode)!,
    }
    if allocator.resize(block, empty_b) {
        return (17 as process::ExitCode)!;
    }
    switch allocator.remap(block, empty_b) {
        ?moved => {
            if moved.size() != 0 or moved.align() != 16 {
                return (18 as process::ExitCode)!;
            }
        },
        null => return (19 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_from_empty_block() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_from_empty_block");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var empty_layout: mem::Layout;
    switch mem::Layout::init(0, 8) {
        !value => empty_layout = value,
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(empty_layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if block.size() != 0 {
        return (3 as process::ExitCode)!;
    }

    var full_layout: mem::Layout;
    switch mem::Layout::init(16, 8) {
        !value => full_layout = value,
        error! => return (4 as process::ExitCode)!,
    }
    switch allocator.realloc(block, full_layout) {
        !value => block = value,
        error! => return (5 as process::ExitCode)!,
    }
    if block.size() != 16 or block.align() != 8 {
        return (6 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 77u8;
    bytes[15] = 99u8;
    if bytes[0] != 77u8 or bytes[15] != 99u8 {
        return (7 as process::ExitCode)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_zero_sized_and_empty_slice_edges() {
    let root = temp_dir("emit_exe_std_mem_zero_sized_and_empty_slice_edges");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

fn check_allocator_preserves_empty_slice_len() process::ExitCode!void {
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[i32](0) {
        !items => {
            if items.len() != 0 {
                return (2 as process::ExitCode)!;
            }
            switch allocator.free_slice[i32](items) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}

fn check_allocator_preserves_zero_sized_slice_len() process::ExitCode!void {
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[void](4) {
        !items => {
            if items.len() != 4 {
                return (2 as process::ExitCode)!;
            }
            switch allocator.free_slice[void](items) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}

fn check_block_as_slice_handles_zero_sized_element_type() process::ExitCode!void {
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::array[void](8) {
        !value => layout = value,
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    var items = block.as_slice[void]();
    if items.len() != 0 {
        return (3 as process::ExitCode)!;
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    check_allocator_preserves_empty_slice_len().?;
    check_allocator_preserves_zero_sized_slice_len().?;
    check_block_as_slice_handles_zero_sized_element_type().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_copy_forwards_and_backwards() {
    let root = temp_dir("emit_exe_std_mem_copy_forwards_and_backwards");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var left: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_forwards[i32](&mut left[0..3], &left[1..4]);
    let expected_left: [5]i32 = [2, 3, 4, 4, 5];
    if not mem::equal[i32](&left[..], &expected_left[..]) {
        return (1 as process::ExitCode)!;
    }

    var right: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_backwards[i32](&mut right[1..4], &right[0..3]);
    let expected_right: [5]i32 = [1, 1, 2, 3, 5];
    if not mem::equal[i32](&right[..], &expected_right[..]) {
        return (2 as process::ExitCode)!;
    }

    var exact_to: [3]u8 = [0, 0, 0];
    let exact_from: [3]u8 = [7, 8, 9];
    mem::copy_forwards[u8](&mut exact_to[..], &exact_from[..]);
    if not mem::equal[u8](&exact_to[..], &exact_from[..]) {
        return (3 as process::ExitCode)!;
    }

    var short_to: [2]u8 = [0, 0];
    let long_from: [4]u8 = [5, 6, 7, 8];
    mem::copy_forwards[u8](&mut short_to[..], &long_from[..]);
    let expected_short_to: [2]u8 = [5, 6];
    if not mem::equal[u8](&short_to[..], &expected_short_to[..]) {
        return (8 as process::ExitCode)!;
    }

    var short_backward: [2]u8 = [0, 0];
    mem::copy_backwards[u8](&mut short_backward[..], &long_from[..]);
    if not mem::equal[u8](&short_backward[..], &expected_short_to[..]) {
        return (9 as process::ExitCode)!;
    }

    let low: [2]u8 = [1, 2];
    let high: [2]u8 = [1, 3];
    if mem::order[u8](&low[..], &high[..]) != mem::Order::Less {
        return (4 as process::ExitCode)!;
    }
    if mem::order[u8](&high[..], &low[..]) != mem::Order::Greater {
        return (5 as process::ExitCode)!;
    }
    if mem::order[u8](&low[..], &low[..]) != mem::Order::Equal {
        return (6 as process::ExitCode)!;
    }
    let prefix: [1]u8 = [1];
    if mem::order[u8](&prefix[..], &low[..]) != mem::Order::Less {
        return (7 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_memory_intrinsic_builtins() {
    let root = temp_dir("emit_exe_memory_intrinsic_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    var ints: [3]i32 = [0, 0, 0];
    let source_ints: [3]i32 = [7, 8, 9];
    @memcpy(&mut ints[..], &source_ints[..]);
    if ints[0] != 7 or ints[1] != 8 or ints[2] != 9 {
        return (1 as process::ExitCode)!;
    }

    var wide: [5]i32 = [0, 0, 0, 44, 55];
    let short: [3]i32 = [11, 22, 33];
    @memcpy(&mut wide[..], &short[..]);
    if wide[0] != 11 or wide[1] != 22 or wide[2] != 33 or wide[3] != 44 or wide[4] != 55 {
        return (4 as process::ExitCode)!;
    }

    var narrow: [4]u8 = [0, 0, 77, 88];
    let long: [4]u8 = [10, 20, 30, 40];
    @memcpy(&mut narrow[0..2], &long[..]);
    if narrow[0] != 10 or narrow[1] != 20 or narrow[2] != 77 or narrow[3] != 88 {
        return (5 as process::ExitCode)!;
    }

    var overlap: [5]u8 = [1, 2, 3, 4, 5];
    @memmove(&mut overlap[1..], &overlap[0..4]);
    if overlap[0] != 1 or overlap[1] != 1 or overlap[2] != 2 or overlap[3] != 3 or overlap[4] != 4 {
        return (2 as process::ExitCode)!;
    }

    var short_move: [4]u8 = [9, 8, 7, 6];
    @memmove(&mut short_move[0..2], &short_move[1..4]);
    if short_move[0] != 8 or short_move[1] != 7 or short_move[2] != 7 or short_move[3] != 6 {
        return (6 as process::ExitCode)!;
    }

    var bytes: [4]u8 = [1, 2, 3, 4];
    @memset(&mut bytes[1..3], 9);
    if bytes[0] != 1 or bytes[1] != 9 or bytes[2] != 9 or bytes[3] != 4 {
        return (3 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals() {
    let root = temp_dir("emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals");
    let main = root.join("main.nia");
    let helper = root.join("helper.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub fn copy_prefix[T](to: &mut [T], from: &[T]) void
where T: Sized
{
    @memcpy(to, from);
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        &main,
        r#"
using helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var dest: [2]u8 = [0; 2];
    let source: [2]u8 = [b'a', b'b'];
    helper::copy_prefix[u8](&mut dest[..], &source[..]);
    if dest[0] != b'a' or dest[1] != b'b' {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_cross_module_slice_pointee_extension_receiver_passes_slice_value() {
    let root =
        temp_dir("emit_exe_cross_module_slice_pointee_extension_receiver_passes_slice_value");
    let main = root.join("main.nia");
    let helper = root.join("helper.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub struct SliceIter[T]
where T: Sized
{
    ptr: &T,
    len: usize,
    index: usize,
}

extend[T] SliceIter[T]
where T: Sized
{
    pub fn from_raw_parts(ptr: &T, len: usize) SliceIter[T] {
        { ptr: ptr, len: len, index: 0 }
    }
}

extend[T] SliceIter[T] : Iterator
where T: Sized
{
    type Item = &T;

    fn next(&mut self) ?&T {
        if self.index >= self.len {
            null
        } else {
            let item = (self.ptr as usize + self.index * @size[T]()) as &T;
            self.index += 1usize;
            ?item
        }
    }
}

extend[T] [T]
where T: Sized
{
    pub fn iter_custom(&self) SliceIter[T] {
        SliceIter[T]::from_raw_parts(self.get_ptr_read(), self.len())
    }
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        &main,
        r#"
using helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var data: [3]i32 = [10, 20, 30];
    var total = 0;
    for &value in (&data[..]).iter_custom() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_push_pop_and_deinit() {
    let root = temp_dir("emit_exe_std_array_list_push_pop_and_deinit");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var exact: std::ArrayList[i32];
    switch std::ArrayList[i32]::init_capacity(page, 3) {
        !value => exact = value,
        error! => return (1 as process::ExitCode)!,
    }
    if exact.len() != 0 or exact.capacity() != 3 {
        return (2 as process::ExitCode)!;
    }
    switch exact.deinit(page) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    var ops = std::ArrayList[i32]::init();
    switch ops.push(page, 1) {
        !ok => _ = ok,
        error! => return (26 as process::ExitCode)!,
    }
    switch ops.push(page, 3) {
        !ok => _ = ok,
        error! => return (27 as process::ExitCode)!,
    }
    switch ops.insert(page, 1, 2) {
        !ok => _ = ok,
        error! => return (28 as process::ExitCode)!,
    }
    let inserted_tail: [2]i32 = [4, 5];
    switch ops.insert_slice(page, 3, &inserted_tail[..]) {
        !ok => _ = ok,
        error! => return (29 as process::ExitCode)!,
    }
    let expected_ops: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ops[..]) {
        return (30 as process::ExitCode)!;
    }
    switch ops.ordered_remove(1) {
        ?value => {
            if value != 2 {
                return (31 as process::ExitCode)!;
            }
        },
        null => return (32 as process::ExitCode)!,
    }
    let expected_ordered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ordered[..]) {
        return (33 as process::ExitCode)!;
    }
    switch ops.swap_remove(0) {
        ?value => {
            if value != 1 {
                return (34 as process::ExitCode)!;
            }
        },
        null => return (35 as process::ExitCode)!,
    }
    let expected_swap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.as_slice(), &expected_swap[..]) {
        return (36 as process::ExitCode)!;
    }
    switch ops.deinit(page) {
        !ok => _ = ok,
        error! => return (37 as process::ExitCode)!,
    }

    var alias = std::ArrayList[i32]::init();
    switch alias.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return (38 as process::ExitCode)!,
    }
    switch alias.push(page, 1) {
        !ok => _ = ok,
        error! => return (39 as process::ExitCode)!,
    }
    switch alias.push(page, 2) {
        !ok => _ = ok,
        error! => return (40 as process::ExitCode)!,
    }
    switch alias.append_slice(page, alias.as_slice()) {
        !ok => _ = ok,
        error! => return (41 as process::ExitCode)!,
    }
    let expected_alias_append: [4]i32 = [1, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_append[..]) {
        return (42 as process::ExitCode)!;
    }
    switch alias.insert_slice(page, 1, alias.as_slice()) {
        !ok => _ = ok,
        error! => return (43 as process::ExitCode)!,
    }
    let expected_alias_insert: [8]i32 = [1, 1, 2, 1, 2, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_insert[..]) {
        return (44 as process::ExitCode)!;
    }
    switch alias.deinit(page) {
        !ok => _ = ok,
        error! => return (45 as process::ExitCode)!,
    }

    var list = std::ArrayList[i32]::init();
    if list.len() != 0 or not list.is_empty() {
        return (4 as process::ExitCode)!;
    }
    switch list.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    if list.capacity() != 2 {
        return (6 as process::ExitCode)!;
    }
    switch list.reserve(page, 3) {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    if list.capacity() < 5 {
        return (8 as process::ExitCode)!;
    }
    var index = 0;
    while index < 6 {
        switch list.push(page, index * 10) {
            !ok => _ = ok,
            error! => return (9 as process::ExitCode)!,
        }
        index += 1;
    }
    if list.len() != 6 or list.capacity() < 6 {
        return (10 as process::ExitCode)!;
    }
    let items = list.as_slice();
    if items[0] != 0 or items[1] != 10 or items[5] != 50 {
        return (11 as process::ExitCode)!;
    }
    switch list.first() {
        ?value => {
            if value.* != 0 {
                return (64 as process::ExitCode)!;
            }
        },
        null => return (65 as process::ExitCode)!,
    }
    switch list.last() {
        ?value => {
            if value.* != 50 {
                return (66 as process::ExitCode)!;
            }
        },
        null => return (67 as process::ExitCode)!,
    }
    switch list.get(3) {
        ?value => {
            if value.* != 30 {
                return (68 as process::ExitCode)!;
            }
        },
        null => return (69 as process::ExitCode)!,
    }
    switch list.get(6) {
        ?value => {
            _ = value;
            return (70 as process::ExitCode)!;
        },
        null => {},
    }
    switch list.get_mut(4) {
        ?value => value.* = 44,
        null => return (71 as process::ExitCode)!,
    }
    switch list.last_mut() {
        ?value => value.* = 55,
        null => return (72 as process::ExitCode)!,
    }
    let expected_after_accessors: [6]i32 = [0, 10, 20, 30, 44, 55];
    if not mem::equal[i32](list.as_slice(), &expected_after_accessors[..]) {
        return (73 as process::ExitCode)!;
    }

    let more: [3]i32 = [60, 70, 80];
    switch list.append_slice(page, &more[..]) {
        !ok => _ = ok,
        error! => return (12 as process::ExitCode)!,
    }
    if list.len() != 9 or list.as_slice()[8] != 80 {
        return (13 as process::ExitCode)!;
    }

    switch list.add_one(page) {
        !slot => slot.* = 90,
        error! => return (14 as process::ExitCode)!,
    }
    if list.len() != 10 or list.as_slice()[9] != 90 {
        return (15 as process::ExitCode)!;
    }

    switch list.add_many_as_slice(page, 2) {
        !slots => {
            slots[0] = 100;
            slots[1] = 110;
        },
        error! => return (16 as process::ExitCode)!,
    }
    if list.len() != 12 or list.as_slice()[11] != 110 {
        return (17 as process::ExitCode)!;
    }

    switch list.add_many_at(page, 2, 2) {
        !slots => {
            slots[0] = 21;
            slots[1] = 22;
        },
        error! => return (46 as process::ExitCode)!,
    }
    if list.len() != 14 or list.as_slice()[2] != 21 or list.as_slice()[3] != 22 or list.as_slice()[4] != 20 {
        return (47 as process::ExitCode)!;
    }

    list.append_assume_capacity(120);
    if list.len() != 15 or list.as_slice()[14] != 120 {
        return (48 as process::ExitCode)!;
    }

    switch list.resize(page, 18) {
        !ok => _ = ok,
        error! => return (49 as process::ExitCode)!,
    }
    if list.len() != 18 {
        return (50 as process::ExitCode)!;
    }
    var unused = list.unused_capacity_slice();
    if unused.len() < 2 {
        return (51 as process::ExitCode)!;
    }
    unused[0] = 180;
    unused[1] = 190;
    switch list.add_many_as_slice(page, 2) {
        !slots => {
            if slots[0] != 180 or slots[1] != 190 {
                return (52 as process::ExitCode)!;
            }
        },
        error! => return (53 as process::ExitCode)!,
    }
    if list.len() != 20 {
        return (54 as process::ExitCode)!;
    }

    switch list.resize(page, 12) {
        !ok => _ = ok,
        error! => return (55 as process::ExitCode)!,
    }
    if list.len() != 12 {
        return (56 as process::ExitCode)!;
    }

    let before_shrink_capacity = list.capacity();
    switch list.shrink_to_len(page) {
        !ok => _ = ok,
        error! => return (57 as process::ExitCode)!,
    }
    if list.len() != 12 or list.capacity() > before_shrink_capacity or list.capacity() < list.len() {
        return (58 as process::ExitCode)!;
    }

    switch list.reserve_exact(page, 4) {
        !ok => _ = ok,
        error! => return (59 as process::ExitCode)!,
    }
    list.expand_to_capacity();
    if list.len() != list.capacity() {
        return (60 as process::ExitCode)!;
    }

    switch list.shrink_and_free(page, 10) {
        !ok => _ = ok,
        error! => return (61 as process::ExitCode)!,
    }
    if list.len() != 10 or list.capacity() < 10 {
        return (62 as process::ExitCode)!;
    }

    let allocated = list.allocated_slice();
    if allocated.len() != list.capacity() {
        return (63 as process::ExitCode)!;
    }

    let retained_capacity = list.capacity();
    list.shrink_retaining_capacity(10);
    if list.len() != 10 or list.capacity() != retained_capacity {
        return (18 as process::ExitCode)!;
    }

    switch list.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return (74 as process::ExitCode)!,
    }
    let tail: [2]i32 = [100, 110];
    list.append_slice_assume_capacity(&tail[..]);
    if list.len() != 12 or list.as_slice()[10] != 100 or list.as_slice()[11] != 110 {
        return (19 as process::ExitCode)!;
    }

    switch list.pop() {
        ?value => {
            if value != 110 {
                return (20 as process::ExitCode)!;
            }
        },
        null => return (21 as process::ExitCode)!,
    }
    if list.len() != 11 {
        return (22 as process::ExitCode)!;
    }
    var mutable_items = list.as_mut_slice();
    mutable_items[2] = 77;
    if list.as_slice()[2] != 77 {
        return (23 as process::ExitCode)!;
    }
    list.clear_retaining_capacity();
    if not list.is_empty() {
        return (24 as process::ExitCode)!;
    }
    switch list.clear_and_free(page) {
        !ok => _ = ok,
        error! => return (25 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_can_shrink_to_zero_capacity_and_reuse() {
    let root = temp_dir("emit_exe_std_array_list_can_shrink_to_zero_capacity_and_reuse");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var list = std::ArrayList[i32]::init();
    switch list.push(page, 10) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    switch list.push(page, 20) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch list.shrink_to_capacity(page, 0) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    if list.len() != 0 or list.capacity() != 0 {
        return (4 as process::ExitCode)!;
    }

    switch list.push(page, 30) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    switch list.push(page, 40) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    let expected: [2]i32 = [30, 40];
    if not mem::equal[i32](list.as_slice(), &expected[..]) {
        return (7 as process::ExitCode)!;
    }
    switch list.deinit(page) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_owned_slice_and_clone() {
    let root = temp_dir("emit_exe_std_array_list_owned_slice_and_clone");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;

    var source = std::ArrayList[i32]::init();
    switch source.push(page, 1) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    switch source.push(page, 2) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }

    var cloned: std::ArrayList[i32];
    switch source.clone(page) {
        !value => cloned = value,
        error! => return (3 as process::ExitCode)!,
    }
    var source_items = source.as_mut_slice();
    source_items[0] = 9;
    let expected_source: [2]i32 = [9, 2];
    let expected_clone: [2]i32 = [1, 2];
    if not mem::equal[i32](source.as_slice(), &expected_source[..]) {
        return (4 as process::ExitCode)!;
    }
    if not mem::equal[i32](cloned.as_slice(), &expected_clone[..]) {
        return (5 as process::ExitCode)!;
    }

    var owned: &mut [i32];
    switch source.into_owned_slice(page) {
        !value => owned = value,
        error! => return (6 as process::ExitCode)!,
    }
    if source.len() != 0 or source.capacity() != 0 {
        return (7 as process::ExitCode)!;
    }
    if not mem::equal[i32](owned, &expected_source[..]) {
        return (8 as process::ExitCode)!;
    }
    switch page.free_slice[i32](owned) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }

    var external: &mut [i32];
    switch page.alloc_slice[i32](3) {
        !items => external = items,
        error! => return (10 as process::ExitCode)!,
    }
    external[0] = 4;
    external[1] = 5;
    external[2] = 6;
    var adopted = std::ArrayList[i32]::from_owned_slice(external);
    let expected_adopted: [3]i32 = [4, 5, 6];
    if adopted.capacity() != 3 or not mem::equal[i32](adopted.as_slice(), &expected_adopted[..]) {
        return (11 as process::ExitCode)!;
    }
    switch adopted.deinit(page) {
        !ok => _ = ok,
        error! => return (12 as process::ExitCode)!,
    }
    switch cloned.deinit(page) {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_handles_zero_sized_elements_without_allocation() {
    let root = temp_dir("emit_exe_std_array_list_handles_zero_sized_elements_without_allocation");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

struct Marker {}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var list = std::ArrayList[Marker]::init();
    switch list.reserve(page, 4) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    if list.capacity() != usize::MAX {
        return (2 as process::ExitCode)!;
    }
    switch list.push(page, {}) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch list.resize(page, 16) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }
    if list.len() != 16 or list.capacity() != usize::MAX {
        return (5 as process::ExitCode)!;
    }
    switch list.shrink_and_free(page, 3) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    if list.len() != 3 or list.capacity() != usize::MAX {
        return (7 as process::ExitCode)!;
    }
    switch list.deinit(page) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
