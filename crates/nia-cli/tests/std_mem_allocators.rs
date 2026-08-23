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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 256] = [0; 256];
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
        return process::exit(1)!;
    }
    if allocator.used() == 0 {
        return process::exit(2)!;
    }

    list.deinit(&mut allocator).exit().?;
    allocator.reset();
    if allocator.used() != 0 or allocator.remaining() != allocator.capacity() {
        return process::exit(3)!;
    }

    let mut tiny: [u8; 8] = [0; 8];
    let mut failing = mem::FixedBufferAllocator::init(&mut tiny[..]);
    match failing.allocBytes(16, 1) {
        !block => { _ = block;
                return process::exit(4)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                    return process::exit(5)!;
                } },
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

pub fn main(init: process::Init) process::ExitCode!() {
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
        return process::exit(1)!;
    }

    let capacity = arena.capacity();
    if capacity == 0 or arena.used() == 0 {
        return process::exit(2)!;
    }

    arena.reset().exit().?;
    if arena.capacity() != capacity or arena.used() != 0 {
        return process::exit(3)!;
    }

    let mut bytes = arena.allocSlice[u8](64).exit().?;
    bytes.asMutSlice()[0] = 7;
    bytes.asMutSlice()[63] = 9;
    if bytes.asSlice()[0] != 7 or bytes.asSlice()[63] != 9 {
        return process::exit(4)!;
    }
    bytes.deinit(&mut arena).exit().?;

    arena.deinit().exit().?;
    if arena.capacity() != 0 or arena.used() != 0 {
        return process::exit(5)!;
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
fn emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges() {
    let root = temp_dir("emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    let mut first = arena.allocBytes(16, 8).exit().?;
    let mut second = arena.allocBytes(16, 8).exit().?;
    if arena.resize(first, mem::Layout::init(32, 8).exit().?) {
        return process::exit(1)!;
    }
    if not arena.resize(first, mem::Layout::init(8, 8).exit().?) {
        return process::exit(2)!;
    }

    match arena.remap(second, mem::Layout::init(40, 8).exit().?) {
        ?grown => { second = grown; },
        null => { return process::exit(3)!; },
    }
    if second.size() != 40 {
        return process::exit(4)!;
    }

    arena.free(second).exit().?;
    match arena.allocBytes(40, 8) {
        !again => { if again.ptr() as usize != second.ptr() as usize {
                    return process::exit(5)!;
                } },
        error! => { return process::exit(6)!; },
    }

    let retainedCapacity = arena.capacity();
    arena.reset().exit().?;
    if arena.capacity() != retainedCapacity or arena.used() != 0 {
        return process::exit(7)!;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(24, 8).exit().?;
    let mut first = allocator.alloc(layout).exit().?;
    let mut second = allocator.alloc(layout).exit().?;
    if first.ptr() as usize == second.ptr() as usize {
        return process::exit(1)!;
    }

    let first_addr = first.ptr() as usize;
    allocator.free(first).exit().?;
    let mut reused = allocator.alloc(layout).exit().?;
    if reused.ptr() as usize != first_addr {
        return process::exit(2)!;
    }

    allocator.free(reused).exit().?;
    allocator.free(second).exit().?;
    if not allocator.isEmpty() {
        return process::exit(3)!;
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
        return process::exit(4)!;
    }
    if allocator.used() == 0 or allocator.capacity() == 0 {
        return process::exit(5)!;
    }
    list.deinit(&mut allocator).exit().?;
    allocator.deinit().ok().exit().?;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(3000, 4096).exit().?;
    let mut block = allocator.alloc(layout).exit().?;
    if block.ptr() as usize % 4096 != 0 {
        return process::exit(1)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 11;
    bytes[2999] = 22;

    let grown_layout = mem::Layout::init(3040, 4096).exit().?;
    let old_addr = block.ptr() as usize;
    block = allocator.realloc(block, grown_layout).exit().?;
    if block.ptr() as usize != old_addr or block.size() != 3040 {
        return process::exit(2)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11 or bytes[2999] != 22 {
        return process::exit(3)!;
    }

    let moved_layout = mem::Layout::init(12000, 4096).exit().?;
    block = allocator.realloc(block, moved_layout).exit().?;
    if block.ptr() as usize % 4096 != 0 or block.size() != 12000 {
        return process::exit(4)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11 or bytes[2999] != 22 {
        return process::exit(5)!;
    }

    let empty_layout = mem::Layout::init(0, 4096).exit().?;
    block = allocator.realloc(block, empty_layout).exit().?;
    if not block.isEmpty() or not allocator.isEmpty() {
        return process::exit(6)!;
    }
    allocator.free(block).exit().?;
    if not allocator.isEmpty() {
        return process::exit(7)!;
    }
    allocator.deinit().ok().exit().?;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let small_layout = mem::Layout::init(32, 8).exit().?;
    let small = allocator.alloc(small_layout).exit().?;
    allocator.free(small).exit().?;
    match allocator.free(small) {
        !ok => { _ = ok;
                return process::exit(1)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::exit(2)!;
                } },
    }
    if allocator.resize(small, small_layout) {
        return process::exit(3)!;
    }

    let mut resized = allocator.alloc(small_layout).exit().?;
    let resized_layout = mem::Layout::init(40, 8).exit().?;
    if not allocator.resize(resized, resized_layout) {
        return process::exit(7)!;
    }
    if allocator.used() != 40 {
        return process::exit(8)!;
    }
    match allocator.free(resized) {
        !ok => { _ = ok;
                return process::exit(9)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::exit(10)!;
                } },
    }
    resized = mem::Block::init(resized.ptr(), resized_layout);
    allocator.free(resized).exit().?;

    let align_layout = mem::Layout::init(1, 1).exit().?;
    let align_block = allocator.alloc(align_layout).exit().?;
    if allocator.resize(align_block, mem::Layout::init(1, 2).exit().?) {
        return process::exit(11)!;
    }
    allocator.free(align_block).exit().?;

    let large_layout = mem::Layout::init(4096, 4096).exit().?;
    let large = allocator.alloc(large_layout).exit().?;
    let wrong_layout = mem::Layout::init(2048, 4096).exit().?;
    let wrong = mem::Block::init(large.ptr(), wrong_layout);
    match allocator.free(wrong) {
        !ok => { _ = ok;
                return process::exit(4)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::exit(5)!;
                } },
    }
    allocator.free(large).exit().?;
    if not allocator.isEmpty() {
        return process::exit(6)!;
    }
    allocator.deinit().ok().exit().?;
    let mut leaking = mem::GeneralPurposeAllocator::init(&mut page);
    _ = leaking.alloc(small_layout).exit().?;
    if leaking.deinit().exit().? != mem::DeinitStatus::Leak {
        return process::exit(13)!;
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
fn emit_exe_std_mem_allocator_realloc_preserves_byte_prefix() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_preserves_byte_prefix");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut block: mem::Block;
    match allocator.allocBytes(4, 1) {
        !value => { block = value; },
        error! => { return process::exit(1)!; },
    }
    let mut bytes = block.bytes();
    bytes[0] = 10;
    bytes[1] = 20;
    bytes[2] = 30;
    bytes[3] = 40;

    let mut grow_layout: mem::Layout;
    match mem::Layout::init(8, 1) {
        !value => { grow_layout = value; },
        error! => { return process::exit(2)!; },
    }
    match allocator.realloc(block, grow_layout) {
        !value => { block = value; },
        error! => { return process::exit(3)!; },
    }
    if block.size() != 8 {
        return process::exit(4)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10 or bytes[1] != 20 or bytes[2] != 30 or bytes[3] != 40 {
        return process::exit(5)!;
    }
    bytes[4] = 50;
    bytes[5] = 60;

    let mut shrink_layout: mem::Layout;
    match mem::Layout::init(2, 1) {
        !value => { shrink_layout = value; },
        error! => { return process::exit(6)!; },
    }
    match allocator.realloc(block, shrink_layout) {
        !value => { block = value; },
        error! => { return process::exit(7)!; },
    }
    if block.size() != 2 {
        return process::exit(8)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10 or bytes[1] != 20 {
        return process::exit(9)!;
    }

    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::exit(10)!; },
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
fn emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::math;
using std::process;

struct CountingAllocator {
    buffer: &mut [u8],
    end_index: usize,
    free_count: usize,
    fail_free_count: usize,
}

extend CountingAllocator {
    fn init(buffer: &mut [u8]) CountingAllocator {
        Self {
            buffer,
            end_index: 0,
            free_count: 0,
            fail_free_count: 0,
        }
    }

    fn failNextFrees(&mut self, count: usize) () {
        self.fail_free_count = count;
    }
}

extend CountingAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if layout.isEmpty() {
            return !mem::Block::init(layout.align() as &mut u8, layout);
        }
        let base = self.buffer.ptrMut() as usize;
        let current = base + self.end_index;
        let aligned = match current.align_forward(layout.align()) {
            ?value => { value },
            null => { return mem::Error::OutOfMemory!; },
        };
        let offset = aligned - base;
        let next = match offset.checked_add(layout.size()) {
            ?value => { value },
            null => { return mem::Error::OutOfMemory!; },
        };
        if next > self.buffer.len() {
            return mem::Error::OutOfMemory!;
        }
        self.end_index = next;
        !mem::Block::init(aligned as &mut u8, layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if block.isEmpty() {
            return !();
        }
        self.free_count += 1;
        if self.fail_free_count != 0 {
            self.fail_free_count -= 1;
            return mem::Error::Invalid!;
        }
        !()
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 64] = [0; 64];
    let mut allocator = CountingAllocator::init(&mut storage);
    let old_layout = mem::Layout::init(4, 1).exit().?;
    let new_layout = mem::Layout::init(8, 1).exit().?;
    let mut block = allocator.alloc(old_layout).exit().?;
    let mut bytes = block.bytes();
    bytes[0] = 10;
    bytes[1] = 20;

    allocator.failNextFrees(1);
    match allocator.realloc(block, new_layout) {
        !new_block => { _ = new_block;
                return process::exit(1)!; },
        mem::ReallocError::OldFree(cause)! => {
            if cause as i32 != mem::Error::Invalid as i32 {
                return process::exit(2)!;
            }
        },
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }

    if allocator.free_count != 2 {
        return process::exit(3)!;
    }

    let second_block = allocator.alloc(old_layout).exit().?;
    allocator.failNextFrees(2);
    match allocator.realloc(second_block, new_layout) {
        !new_block => {
            _ = new_block;
            return process::exit(4)!;
        },
        mem::ReallocError::Rollback {
            primary,
            replacement,
            replacementError,
        }! => {
            if primary as i32 != mem::Error::Invalid as i32
                or replacementError as i32 != mem::Error::Invalid as i32
                or replacement.size() != new_layout.size()
            {
                return process::exit(5)!;
            }
            allocator.free(replacement).exit().?;
        },
        error! => {
            _ = error;
            return process::exit(6)!;
        },
    }
    allocator.free(second_block).exit().?;
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
fn emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics() {
    let root = temp_dir("emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    match mem::Layout::init(16, 8) {
        !value => { layout = value; },
        error! => { return process::exit(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(layout) {
        !value => { block = value; },
        error! => { return process::exit(2)!; },
    }
    if not allocator.resize(block, layout) {
        return process::exit(3)!;
    }

    let mut larger: mem::Layout;
    match mem::Layout::init(32, 8) {
        !value => { larger = value; },
        error! => { return process::exit(4)!; },
    }
    if not allocator.resize(block, larger) {
        return process::exit(5)!;
    }
    match allocator.remap(block, larger) {
        ?same => { if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                    return process::exit(6)!;
                }
                block = same; },
        null => { return process::exit(7)!; },
    }
    match allocator.remap(block, layout) {
        ?same => { if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                    return process::exit(8)!;
                }
                block = same; },
        null => { return process::exit(9)!; },
    }

    let mut next_page: mem::Layout;
    match mem::Layout::init(8192, 8) {
        !value => { next_page = value; },
        error! => { return process::exit(10)!; },
    }
    if allocator.resize(block, next_page) {
        return process::exit(11)!;
    }
    match allocator.remap(block, next_page) {
        ?moved => { _ = moved;
                return process::exit(12)!; },
        null => { },
    }
    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::exit(13)!; },
    }

    let mut empty_a: mem::Layout;
    match mem::Layout::init(0, 8) {
        !value => { empty_a = value; },
        error! => { return process::exit(14)!; },
    }
    match allocator.alloc(empty_a) {
        !value => { block = value; },
        error! => { return process::exit(15)!; },
    }
    let mut empty_b: mem::Layout;
    match mem::Layout::init(0, 16) {
        !value => { empty_b = value; },
        error! => { return process::exit(16)!; },
    }
    if allocator.resize(block, empty_b) {
        return process::exit(17)!;
    }
    match allocator.remap(block, empty_b) {
        ?moved => { if moved.size() != 0 or moved.align() != 16 {
                    return process::exit(18)!;
                } },
        null => { return process::exit(19)!; },
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
fn emit_exe_std_mem_default_remap_preserves_release_range() {
    let root = temp_dir("emit_exe_std_mem_default_remap_preserves_release_range");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct ResizeOnlyPageAllocator {
    page: mem::PageAllocator,
}

extend ResizeOnlyPageAllocator {
    fn init() ResizeOnlyPageAllocator {
        Self { page: mem::PageAllocator::init() }
    }
}

extend ResizeOnlyPageAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        self.page.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        self.page.free(block)
    }

    fn resize(&mut self, block: mem::Block, newLayout: mem::Layout) bool {
        block.align() == newLayout.align()
            and block.size() != 0
            and newLayout.size() != 0
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = ResizeOnlyPageAllocator::init();
    let initial = mem::Layout::init(64, 8192).exit().?;
    let resized = mem::Layout::init(32, 8192).exit().?;
    let block = allocator.alloc(initial).exit().?;
    if block.ptr() as usize % 8192 != 0 {
        return process::exit(1)!;
    }
    let remapped = match allocator.remap(block, resized) {
        ?value => value,
        null => return process::exit(2)!,
    };
    if remapped.ptr() as usize != block.ptr() as usize or remapped.size() != 32 {
        return process::exit(3)!;
    }
    allocator.free(remapped).exit().?;
    !()
}
"#,
    )
    .expect("write default remap release owner source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile default remap release owner");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run default remap release owner");
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut empty_layout: mem::Layout;
    match mem::Layout::init(0, 8) {
        !value => { empty_layout = value; },
        error! => { return process::exit(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(empty_layout) {
        !value => { block = value; },
        error! => { return process::exit(2)!; },
    }
    if block.size() != 0 {
        return process::exit(3)!;
    }

    let mut full_layout: mem::Layout;
    match mem::Layout::init(16, 8) {
        !value => { full_layout = value; },
        error! => { return process::exit(4)!; },
    }
    match allocator.realloc(block, full_layout) {
        !value => { block = value; },
        error! => { return process::exit(5)!; },
    }
    if block.size() != 16 or block.align() != 8 {
        return process::exit(6)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 77;
    bytes[15] = 99;
    if bytes[0] != 77 or bytes[15] != 99 {
        return process::exit(7)!;
    }

    match allocator.free(block) {
        !ok => { _ = ok; },
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
fn emit_exe_std_mem_general_allocator_rejects_wrapped_large_header_base() {
    let root = temp_dir("emit_exe_std_mem_general_allocator_rejects_wrapped_large_header_base");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct MalformedAllocator {
    failNextFree: bool,
}

extend MalformedAllocator {
    fn init() MalformedAllocator {
        Self { failNextFree: true }
    }
}

extend MalformedAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if layout.isEmpty() {
            return !mem::Block::empty(layout);
        }
        !mem::Block::init((usize::MAX - 1usize) as &mut u8, layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        _ = block;
        if self.failNextFree {
            self.failNextFree = false;
            return mem::Error::Invalid!;
        }
        !()
    }

    fn resize(&mut self, block: mem::Block, newLayout: mem::Layout) bool {
        _ = block;
        _ = newLayout;
        false
    }

    fn remap(&mut self, block: mem::Block, newLayout: mem::Layout) ?mem::Block {
        _ = block;
        _ = newLayout;
        null
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut child = MalformedAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut child);
    let layout = mem::Layout::init(4096, 8).?;
    match allocator.alloc(layout) {
        !block => { _ = block;
                return process::exit(1)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return process::exit(2)!;
            } },
    }
    if allocator.isEmpty() or allocator.capacity() == 0 {
        return process::exit(3)!;
    }
    allocator.deinitWithoutLeakCheck().?;
    if not allocator.isEmpty() {
        return process::exit(4)!;
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
