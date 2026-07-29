// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

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
    let mut storage: [256]u8 = [_]u8[0; 256];
    let mut allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    let mut list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    let mut total = 0;
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

    let mut tiny: [8]u8 = [_]u8[0; 8];
    let mut failing = mem::FixedBufferAllocator::init(&mut tiny[..]);
    if !block = failing.alloc_bytes(16, 1) { _ = block;
            return (4 as process::ExitCode)!; } or err! { if err as i32 != mem::Error::OutOfMemory as i32 {
                return (5 as process::ExitCode)!;
            } }
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
    let mut page = mem::PageAllocator::init();
    let mut arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    let mut list = std::ArrayList[i32]::init();
    list.push(&mut arena, 10).exit().?;
    list.push(&mut arena, 20).exit().?;
    list.push(&mut arena, 30).exit().?;

    let mut total = 0;
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

    let mut bytes = arena.alloc_slice[u8](64).exit().?;
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
    let mut page = mem::PageAllocator::init();
    let mut arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    let mut first = arena.alloc_bytes(16, 8).exit().?;
    let mut second = arena.alloc_bytes(16, 8).exit().?;
    if arena.resize(first, mem::Layout::init(32, 8).exit().?) {
        return (1 as process::ExitCode)!;
    }
    if not arena.resize(first, mem::Layout::init(8, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }

    if ?grown = arena.remap(second, mem::Layout::init(40, 8).exit().?) { second = grown; } or null { return (3 as process::ExitCode)!; }
    if second.size() != 40 {
        return (4 as process::ExitCode)!;
    }

    arena.free(second).exit().?;
    if !again = arena.alloc_bytes(40, 8) { if again.ptr() as usize != second.ptr() as usize {
                return (5 as process::ExitCode)!;
            } } or error! { return (6 as process::ExitCode)!; }

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
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(24, 8).exit().?;
    let mut first = allocator.alloc(layout).exit().?;
    let mut second = allocator.alloc(layout).exit().?;
    if first.ptr() as usize == second.ptr() as usize {
        return (1 as process::ExitCode)!;
    }

    let first_addr = first.ptr() as usize;
    allocator.free(first).exit().?;
    let mut reused = allocator.alloc(layout).exit().?;
    if reused.ptr() as usize != first_addr {
        return (2 as process::ExitCode)!;
    }

    allocator.free(reused).exit().?;
    allocator.free(second).exit().?;
    if not allocator.is_empty() {
        return (3 as process::ExitCode)!;
    }

    let mut list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    let mut total = 0;
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
    allocator.deinit().ok().exit().?;
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
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(3000, 4096).exit().?;
    let mut block = allocator.alloc(layout).exit().?;
    if block.ptr() as usize % 4096usize != 0usize {
        return (1 as process::ExitCode)!;
    }
    let mut bytes = block.bytes();
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
    allocator.deinit().ok().exit().?;
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
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let small_layout = mem::Layout::init(32, 8).exit().?;
    let small = allocator.alloc(small_layout).exit().?;
    allocator.free(small).exit().?;
    if !ok = allocator.free(small) { _ = ok;
            return (1 as process::ExitCode)!; } or err! { if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            } }
    if allocator.resize(small, small_layout) {
        return (3 as process::ExitCode)!;
    }

    let mut resized = allocator.alloc(small_layout).exit().?;
    let resized_layout = mem::Layout::init(40, 8).exit().?;
    if not allocator.resize(resized, resized_layout) {
        return (7 as process::ExitCode)!;
    }
    if allocator.query_used() != 40usize {
        return (8 as process::ExitCode)!;
    }
    if !ok = allocator.free(resized) { _ = ok;
            return (9 as process::ExitCode)!; } or err! { if err as i32 != mem::Error::Invalid as i32 {
                return (10 as process::ExitCode)!;
            } }
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
    if !ok = allocator.free(wrong) { _ = ok;
            return (4 as process::ExitCode)!; } or err! { if err as i32 != mem::Error::Invalid as i32 {
                return (5 as process::ExitCode)!;
            } }
    allocator.free(large).exit().?;
    if not allocator.is_empty() {
        return (6 as process::ExitCode)!;
    }
    allocator.deinit().ok().exit().?;
    let mut leaking = mem::GeneralPurposeAllocator::init(&mut page);
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
    let mut allocator = mem::PageAllocator::init();
    let mut block: mem::Block;
    if !value = allocator.alloc_bytes(4, 1) { block = value; } or error! { return (1 as process::ExitCode)!; }
    let mut bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;
    bytes[2] = 30u8;
    bytes[3] = 40u8;

    let mut grow_layout: mem::Layout;
    if !value = mem::Layout::init(8, 1) { grow_layout = value; } or error! { return (2 as process::ExitCode)!; }
    if !value = allocator.realloc(block, grow_layout) { block = value; } or error! { return (3 as process::ExitCode)!; }
    if block.size() != 8 {
        return (4 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 or bytes[2] != 30u8 or bytes[3] != 40u8 {
        return (5 as process::ExitCode)!;
    }
    bytes[4] = 50u8;
    bytes[5] = 60u8;

    let mut shrink_layout: mem::Layout;
    if !value = mem::Layout::init(2, 1) { shrink_layout = value; } or error! { return (6 as process::ExitCode)!; }
    if !value = allocator.realloc(block, shrink_layout) { block = value; } or error! { return (7 as process::ExitCode)!; }
    if block.size() != 2 {
        return (8 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 {
        return (9 as process::ExitCode)!;
    }

    if !ok = allocator.free(block) { _ = ok; } or error! { return (10 as process::ExitCode)!; }
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
        let base = self.buffer.ptr_mut() as usize;
        let current = base + self.end_index;
        let aligned = if ?value = current.align_forward(layout.align()) { value } or null { return mem::Error::OutOfMemory!; };
        let offset = aligned - base;
        let next = if ?value = offset.checked_add(layout.size()) { value } or null { return mem::Error::OutOfMemory!; };
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
    let mut storage: [64]u8 = [_]u8[0; 64];
    let mut allocator = CountingAllocator::init(&mut storage);
    let old_layout = mem::Layout::init(4, 1).exit().?;
    let new_layout = mem::Layout::init(8, 1).exit().?;
    let mut block = allocator.alloc(old_layout).exit().?;
    let mut bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;

    if !new_block = allocator.realloc(block, new_layout) { _ = new_block;
            return (1 as process::ExitCode)!; } or err! { if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            } }

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
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    if !value = mem::Layout::init(16, 8) { layout = value; } or error! { return (1 as process::ExitCode)!; }
    let mut block: mem::Block;
    if !value = allocator.alloc(layout) { block = value; } or error! { return (2 as process::ExitCode)!; }
    if not allocator.resize(block, layout) {
        return (3 as process::ExitCode)!;
    }

    let mut larger: mem::Layout;
    if !value = mem::Layout::init(32, 8) { larger = value; } or error! { return (4 as process::ExitCode)!; }
    if not allocator.resize(block, larger) {
        return (5 as process::ExitCode)!;
    }
    if ?same = allocator.remap(block, larger) { if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                return (6 as process::ExitCode)!;
            }
            block = same; } or null { return (7 as process::ExitCode)!; }
    if ?same = allocator.remap(block, layout) { if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                return (8 as process::ExitCode)!;
            }
            block = same; } or null { return (9 as process::ExitCode)!; }

    let mut next_page: mem::Layout;
    if !value = mem::Layout::init(8192, 8) { next_page = value; } or error! { return (10 as process::ExitCode)!; }
    if allocator.resize(block, next_page) {
        return (11 as process::ExitCode)!;
    }
    if ?moved = allocator.remap(block, next_page) { _ = moved;
            return (12 as process::ExitCode)!; } or null { }
    if !ok = allocator.free(block) { _ = ok; } or error! { return (13 as process::ExitCode)!; }

    let mut empty_a: mem::Layout;
    if !value = mem::Layout::init(0, 8) { empty_a = value; } or error! { return (14 as process::ExitCode)!; }
    if !value = allocator.alloc(empty_a) { block = value; } or error! { return (15 as process::ExitCode)!; }
    let mut empty_b: mem::Layout;
    if !value = mem::Layout::init(0, 16) { empty_b = value; } or error! { return (16 as process::ExitCode)!; }
    if allocator.resize(block, empty_b) {
        return (17 as process::ExitCode)!;
    }
    if ?moved = allocator.remap(block, empty_b) { if moved.size() != 0 or moved.align() != 16 {
                return (18 as process::ExitCode)!;
            } } or null { return (19 as process::ExitCode)!; }
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
    let mut allocator = mem::PageAllocator::init();
    let mut empty_layout: mem::Layout;
    if !value = mem::Layout::init(0, 8) { empty_layout = value; } or error! { return (1 as process::ExitCode)!; }
    let mut block: mem::Block;
    if !value = allocator.alloc(empty_layout) { block = value; } or error! { return (2 as process::ExitCode)!; }
    if block.size() != 0 {
        return (3 as process::ExitCode)!;
    }

    let mut full_layout: mem::Layout;
    if !value = mem::Layout::init(16, 8) { full_layout = value; } or error! { return (4 as process::ExitCode)!; }
    if !value = allocator.realloc(block, full_layout) { block = value; } or error! { return (5 as process::ExitCode)!; }
    if block.size() != 16 or block.align() != 8 {
        return (6 as process::ExitCode)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 77u8;
    bytes[15] = 99u8;
    if bytes[0] != 77u8 or bytes[15] != 99u8 {
        return (7 as process::ExitCode)!;
    }

    if !ok = allocator.free(block) { _ = ok; } or error! { return (8 as process::ExitCode)!; }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
