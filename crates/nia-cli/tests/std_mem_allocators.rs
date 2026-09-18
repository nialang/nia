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
    list.push(&mut allocator, 10).?;
    list.push(&mut allocator, 20).?;
    list.push(&mut allocator, 30).?;

    let mut total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return process::ExitCode(1)!;
    }
    if allocator.used() == 0 {
        return process::ExitCode(2)!;
    }

    list.deinit(&mut allocator).?;
    allocator.reset();
    if allocator.used() != 0 or allocator.remaining() != allocator.capacity() {
        return process::ExitCode(3)!;
    }

    let mut tiny: [u8; 8] = [0; 8];
    let mut failing = mem::FixedBufferAllocator::init(&mut tiny[..]);
    match failing.allocBytes(16, 1) {
        !block => { _ = block;
                return process::ExitCode(4)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                    return process::ExitCode(5)!;
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
    defer arena.deinit().?;

    let mut list = std::ArrayList[i32]::init();
    list.push(&mut arena, 10).?;
    list.push(&mut arena, 20).?;
    list.push(&mut arena, 30).?;

    let mut total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return process::ExitCode(1)!;
    }

    let capacity = arena.capacity();
    if capacity == 0 or arena.used() == 0 {
        return process::ExitCode(2)!;
    }

    arena.reset().?;
    if arena.capacity() != capacity or arena.used() != 0 {
        return process::ExitCode(3)!;
    }

    let mut bytes = arena.allocSlice[u8](64).?;
    bytes.asMutSlice()[0] = 7;
    bytes.asMutSlice()[63] = 9;
    if bytes.asSlice()[0] != 7 or bytes.asSlice()[63] != 9 {
        return process::ExitCode(4)!;
    }
    bytes.deinit(&mut arena).?;

    arena.deinit().?;
    if arena.capacity() != 0 or arena.used() != 0 {
        return process::ExitCode(5)!;
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
    defer arena.deinit().?;

    let mut first = arena.allocBytes(16, 8).?;
    let mut second = arena.allocBytes(16, 8).?;
    if arena.resize(first, mem::Layout::init(32, 8).?) {
        return process::ExitCode(1)!;
    }
    if not arena.resize(first, mem::Layout::init(8, 8).?) {
        return process::ExitCode(2)!;
    }

    match arena.remap(second, mem::Layout::init(40, 8).?) {
        ?grown => { second = grown; },
        null => { return process::ExitCode(3)!; },
    }
    if second.size() != 40 {
        return process::ExitCode(4)!;
    }

    arena.free(second).?;
    match arena.allocBytes(40, 8) {
        !again => { if again.ptr() as usize != second.ptr() as usize {
                    return process::ExitCode(5)!;
                } },
        error! => { return process::ExitCode(6)!; },
    }

    let retainedCapacity = arena.capacity();
    arena.reset().?;
    if arena.capacity() != retainedCapacity or arena.used() != 0 {
        return process::ExitCode(7)!;
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
fn emit_exe_std_mem_arena_deinit_retries_all_failed_chunks() {
    let root = temp_dir("emit_exe_std_mem_arena_deinit_retries_all_failed_chunks");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct RejectFreeAllocator {
    backing: mem::PageAllocator,
    rejectedFrees: usize,
    liveBlocks: usize,
}

extend RejectFreeAllocator {
    fn init() RejectFreeAllocator {
        Self {
            backing: mem::PageAllocator::init(),
            rejectedFrees: 0,
            liveBlocks: 0,
        }
    }

    fn rejectNext(&mut self, count: usize) () {
        self.rejectedFrees = count;
    }
}

extend RejectFreeAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        let block = self.backing.alloc(layout).?;
        if not block.isEmpty() {
            self.liveBlocks += 1;
        }
        !block
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if self.rejectedFrees != 0usize and not block.isEmpty() {
            self.rejectedFrees -= 1;
            return mem::Error::Invalid!;
        }
        self.backing.free(block).?;
        if not block.isEmpty() {
            self.liveBlocks -= 1;
        }
        !()
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut child = RejectFreeAllocator::init();
    let mut arena = mem::ArenaAllocator::init(&mut child);

    _ = arena.allocBytes(64usize, 8usize).?;
    _ = arena.allocBytes(1024usize * 1024usize, 8usize).?;
    if child.liveBlocks != 2usize {
        return process::ExitCode(1)!;
    }
    arena.reset().?;
    _ = arena.allocBytes(32usize, 8usize).?;
    if arena.used() == 0usize or arena.capacity() == 0usize {
        return process::ExitCode(2)!;
    }

    child.rejectNext(2usize);
    match arena.deinit() {
        !ok => { _ = ok; return process::ExitCode(3)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(4)!; },
    }
    if child.rejectedFrees != 0usize or child.liveBlocks != 2usize {
        return process::ExitCode(5)!;
    }
    if arena.capacity() == 0usize or arena.used() != 0usize {
        return process::ExitCode(6)!;
    }

    arena.deinit().?;
    if child.liveBlocks != 0usize or arena.capacity() != 0usize or arena.used() != 0usize {
        return process::ExitCode(7)!;
    }
    !()
}
"#,
    )
    .expect("write arena cleanup retry source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile arena cleanup retry fixture");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run arena cleanup retry executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_allocator_retries_all_owner_classes() {
    let root = temp_dir("emit_exe_std_mem_general_allocator_retries_all_owner_classes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct MixedOwnerAllocator {
    backing: mem::PageAllocator,
    malformedNext: bool,
    malformedLive: bool,
    rejectedFrees: usize,
    liveBlocks: usize,
}

extend MixedOwnerAllocator {
    fn init() MixedOwnerAllocator {
        Self {
            backing: mem::PageAllocator::init(),
            malformedNext: false,
            malformedLive: false,
            rejectedFrees: 0,
            liveBlocks: 0,
        }
    }

    fn rejectNext(&mut self, count: usize) () {
        self.rejectedFrees = count;
    }
}

extend MixedOwnerAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if self.malformedNext {
            self.malformedNext = false;
            self.malformedLive = true;
            return !mem::Block::init((usize::MAX - 1usize) as &mut u8, layout);
        }
        let block = self.backing.alloc(layout).?;
        if not block.isEmpty() {
            self.liveBlocks += 1;
        }
        !block
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if self.rejectedFrees != 0usize and not block.isEmpty() {
            self.rejectedFrees -= 1;
            return mem::Error::Invalid!;
        }
        if block.ptr() as usize == usize::MAX - 1usize {
            self.malformedLive = false;
            return !();
        }
        self.backing.free(block).?;
        if not block.isEmpty() {
            self.liveBlocks -= 1;
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
    let mut child = MixedOwnerAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut child);

    _ = allocator.alloc(mem::Layout::init(32usize, 8usize).?).?;
    _ = allocator.alloc(mem::Layout::init(4096usize, 4096usize).?).?;
    if child.liveBlocks != 2usize {
        return process::ExitCode(1)!;
    }

    child.malformedNext = true;
    child.rejectNext(1usize);
    match allocator.alloc(mem::Layout::init(4096usize, 8usize).?) {
        !block => { _ = block; return process::ExitCode(2)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(3)!; },
    }
    if not child.malformedLive or allocator.isEmpty() {
        return process::ExitCode(4)!;
    }

    child.rejectNext(3usize);
    match allocator.deinitWithoutLeakCheck() {
        !ok => { _ = ok; return process::ExitCode(5)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(6)!; },
    }
    if child.rejectedFrees != 0usize
        or child.liveBlocks != 2usize
        or not child.malformedLive
        or allocator.capacity() == 0usize
        or allocator.used() == 0usize
    {
        return process::ExitCode(7)!;
    }

    allocator.deinitWithoutLeakCheck().?;
    if child.liveBlocks != 0usize
        or child.malformedLive
        or not allocator.isEmpty()
        or allocator.capacity() != 0usize
        or allocator.used() != 0usize
    {
        return process::ExitCode(8)!;
    }
    !()
}
"#,
    )
    .expect("write general allocator owner cleanup source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile general allocator owner cleanup fixture");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status =
        Command::new(&exe).status_timeout("run general allocator owner cleanup executable");
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

    let layout = mem::Layout::init(24, 8).?;
    let mut first = allocator.alloc(layout).?;
    let mut second = allocator.alloc(layout).?;
    if first.ptr() as usize == second.ptr() as usize {
        return process::ExitCode(1)!;
    }

    let first_addr = first.ptr() as usize;
    allocator.free(first).?;
    let mut reused = allocator.alloc(layout).?;
    if reused.ptr() as usize != first_addr {
        return process::ExitCode(2)!;
    }

    allocator.free(reused).?;
    allocator.free(second).?;
    if not allocator.isEmpty() {
        return process::ExitCode(3)!;
    }

    let mut list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).?;
    list.push(&mut allocator, 20).?;
    list.push(&mut allocator, 30).?;

    let mut total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return process::ExitCode(4)!;
    }
    if allocator.used() == 0 or allocator.capacity() == 0 {
        return process::ExitCode(5)!;
    }
    list.deinit(&mut allocator).?;
    allocator.deinit().ok().?;
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

fn reallocOrExit(
    allocator: &mut mem::Allocator,
    block: mem::Block,
    layout: mem::Layout,
    code: i32,
) process::ExitCode!mem::Block {
    match allocator.realloc(block, layout) {
        !newBlock => !newBlock,
        mem::ReallocError::Allocation(cause)! => {
            _ = cause;
            allocator.free(block).?;
            process::ExitCode(code)!
        },
        mem::ReallocError::OldFree(cause)! => {
            _ = cause;
            allocator.free(block).?;
            process::ExitCode(code)!
        },
        mem::ReallocError::Rollback {
            primary,
            replacement,
            replacementError,
        }! => {
            _ = primary;
            _ = replacementError;
            match allocator.free(replacement) {
                !ok => { _ = ok; },
                error! => { _ = error; },
            }
            match allocator.free(block) {
                !ok => { _ = ok; },
                error! => { _ = error; },
            }
            process::ExitCode(code)!
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page);

    let layout = mem::Layout::init(3000, 4096).?;
    let mut block = allocator.alloc(layout).?;
    if block.ptr() as usize % 4096 != 0 {
        return process::ExitCode(1)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 11;
    bytes[2999] = 22;

    let grown_layout = mem::Layout::init(3040, 4096).?;
    let old_addr = block.ptr() as usize;
    block = reallocOrExit(&mut allocator, block, grown_layout, 8).?;
    if block.ptr() as usize != old_addr or block.size() != 3040 {
        return process::ExitCode(2)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11 or bytes[2999] != 22 {
        return process::ExitCode(3)!;
    }

    let moved_layout = mem::Layout::init(12000, 4096).?;
    block = reallocOrExit(&mut allocator, block, moved_layout, 9).?;
    if block.ptr() as usize % 4096 != 0 or block.size() != 12000 {
        return process::ExitCode(4)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11 or bytes[2999] != 22 {
        return process::ExitCode(5)!;
    }

    let empty_layout = mem::Layout::init(0, 4096).?;
    block = reallocOrExit(&mut allocator, block, empty_layout, 10).?;
    if not block.isEmpty() or not allocator.isEmpty() {
        return process::ExitCode(6)!;
    }
    allocator.free(block).?;
    if not allocator.isEmpty() {
        return process::ExitCode(7)!;
    }
    allocator.deinit().ok().?;
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

    let small_layout = mem::Layout::init(32, 8).?;
    let small = allocator.alloc(small_layout).?;
    allocator.free(small).?;
    match allocator.free(small) {
        !ok => { _ = ok;
                return process::ExitCode(1)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::ExitCode(2)!;
                } },
    }
    if allocator.resize(small, small_layout) {
        return process::ExitCode(3)!;
    }

    let mut resized = allocator.alloc(small_layout).?;
    let resized_layout = mem::Layout::init(40, 8).?;
    if not allocator.resize(resized, resized_layout) {
        return process::ExitCode(7)!;
    }
    if allocator.used() != 40 {
        return process::ExitCode(8)!;
    }
    match allocator.free(resized) {
        !ok => { _ = ok;
                return process::ExitCode(9)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::ExitCode(10)!;
                } },
    }
    resized = mem::Block::init(resized.ptr(), resized_layout);
    allocator.free(resized).?;

    let align_layout = mem::Layout::init(1, 1).?;
    let align_block = allocator.alloc(align_layout).?;
    if allocator.resize(align_block, mem::Layout::init(1, 2).?) {
        return process::ExitCode(11)!;
    }
    allocator.free(align_block).?;

    let large_layout = mem::Layout::init(4096, 4096).?;
    let large = allocator.alloc(large_layout).?;
    let wrong_layout = mem::Layout::init(2048, 4096).?;
    let wrong = mem::Block::init(large.ptr(), wrong_layout);
    match allocator.free(wrong) {
        !ok => { _ = ok;
                return process::ExitCode(4)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::ExitCode(5)!;
                } },
    }
    allocator.free(large).?;
    if not allocator.isEmpty() {
        return process::ExitCode(6)!;
    }
    allocator.deinit().ok().?;
    let mut leaking = mem::GeneralPurposeAllocator::init(&mut page);
    _ = leaking.alloc(small_layout).?;
    if leaking.deinit().? != mem::DeinitStatus::Leak {
        return process::ExitCode(13)!;
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
        error! => { return process::ExitCode(1)!; },
    }
    let mut bytes = block.bytes();
    bytes[0] = 10;
    bytes[1] = 20;
    bytes[2] = 30;
    bytes[3] = 40;

    let mut grow_layout: mem::Layout;
    match mem::Layout::init(8, 1) {
        !value => { grow_layout = value; },
        error! => { return process::ExitCode(2)!; },
    }
    match allocator.realloc(block, grow_layout) {
        !value => { block = value; },
        error! => { return process::ExitCode(3)!; },
    }
    if block.size() != 8 {
        return process::ExitCode(4)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10 or bytes[1] != 20 or bytes[2] != 30 or bytes[3] != 40 {
        return process::ExitCode(5)!;
    }
    bytes[4] = 50;
    bytes[5] = 60;

    let mut shrink_layout: mem::Layout;
    match mem::Layout::init(2, 1) {
        !value => { shrink_layout = value; },
        error! => { return process::ExitCode(6)!; },
    }
    match allocator.realloc(block, shrink_layout) {
        !value => { block = value; },
        error! => { return process::ExitCode(7)!; },
    }
    if block.size() != 2 {
        return process::ExitCode(8)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10 or bytes[1] != 20 {
        return process::ExitCode(9)!;
    }

    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::ExitCode(10)!; },
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
        let aligned = match current.alignForward(layout.align()) {
            ?value => { value },
            null => { return mem::Error::OutOfMemory!; },
        };
        let offset = aligned - base;
        let next = match offset.checkedAdd(layout.size()) {
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
    let old_layout = mem::Layout::init(4, 1).?;
    let new_layout = mem::Layout::init(8, 1).?;
    let mut block = allocator.alloc(old_layout).?;
    let mut bytes = block.bytes();
    bytes[0] = 10;
    bytes[1] = 20;

    allocator.failNextFrees(1);
    match allocator.realloc(block, new_layout) {
        !new_block => { _ = new_block;
                return process::ExitCode(1)!; },
        mem::ReallocError::OldFree(cause)! => {
            if cause as i32 != mem::Error::Invalid as i32 {
                return process::ExitCode(2)!;
            }
        },
        error! => {
            _ = error;
            return process::ExitCode(2)!;
        },
    }

    if allocator.free_count != 2 {
        return process::ExitCode(3)!;
    }

    let second_block = allocator.alloc(old_layout).?;
    allocator.failNextFrees(2);
    match allocator.realloc(second_block, new_layout) {
        !new_block => {
            _ = new_block;
            return process::ExitCode(4)!;
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
                return process::ExitCode(5)!;
            }
            allocator.free(replacement).?;
        },
        error! => {
            _ = error;
            return process::ExitCode(6)!;
        },
    }
    allocator.free(second_block).?;
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
        error! => { return process::ExitCode(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(layout) {
        !value => { block = value; },
        error! => { return process::ExitCode(2)!; },
    }
    if not allocator.resize(block, layout) {
        return process::ExitCode(3)!;
    }

    let mut larger: mem::Layout;
    match mem::Layout::init(32, 8) {
        !value => { larger = value; },
        error! => { return process::ExitCode(4)!; },
    }
    if not allocator.resize(block, larger) {
        return process::ExitCode(5)!;
    }
    match allocator.remap(block, larger) {
        ?same => { if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                    return process::ExitCode(6)!;
                }
                block = same; },
        null => { return process::ExitCode(7)!; },
    }
    match allocator.remap(block, layout) {
        ?same => { if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                    return process::ExitCode(8)!;
                }
                block = same; },
        null => { return process::ExitCode(9)!; },
    }

    let mut next_page: mem::Layout;
    match mem::Layout::init(8192, 8) {
        !value => { next_page = value; },
        error! => { return process::ExitCode(10)!; },
    }
    if allocator.resize(block, next_page) {
        return process::ExitCode(11)!;
    }
    match allocator.remap(block, next_page) {
        ?moved => { _ = moved;
                return process::ExitCode(12)!; },
        null => { },
    }
    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::ExitCode(13)!; },
    }

    let mut empty_a: mem::Layout;
    match mem::Layout::init(0, 8) {
        !value => { empty_a = value; },
        error! => { return process::ExitCode(14)!; },
    }
    match allocator.alloc(empty_a) {
        !value => { block = value; },
        error! => { return process::ExitCode(15)!; },
    }
    let mut empty_b: mem::Layout;
    match mem::Layout::init(0, 16) {
        !value => { empty_b = value; },
        error! => { return process::ExitCode(16)!; },
    }
    if allocator.resize(block, empty_b) {
        return process::ExitCode(17)!;
    }
    match allocator.remap(block, empty_b) {
        ?moved => { if moved.size() != 0 or moved.align() != 16 {
                    return process::ExitCode(18)!;
                } },
        null => { return process::ExitCode(19)!; },
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
    let initial = mem::Layout::init(64, 8192).?;
    let resized = mem::Layout::init(32, 8192).?;
    let block = allocator.alloc(initial).?;
    if block.ptr() as usize % 8192 != 0 {
        return process::ExitCode(1)!;
    }
    let remapped = match allocator.remap(block, resized) {
        ?value => value,
        null => return process::ExitCode(2)!,
    };
    if remapped.ptr() as usize != block.ptr() as usize or remapped.size() != 32 {
        return process::ExitCode(3)!;
    }
    allocator.free(remapped).?;
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
        error! => { return process::ExitCode(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(empty_layout) {
        !value => { block = value; },
        error! => { return process::ExitCode(2)!; },
    }
    if block.size() != 0 {
        return process::ExitCode(3)!;
    }

    let mut full_layout: mem::Layout;
    match mem::Layout::init(16, 8) {
        !value => { full_layout = value; },
        error! => { return process::ExitCode(4)!; },
    }
    match allocator.realloc(block, full_layout) {
        !value => { block = value; },
        error! => { return process::ExitCode(5)!; },
    }
    if block.size() != 16 or block.align() != 8 {
        return process::ExitCode(6)!;
    }
    let mut bytes = block.bytes();
    bytes[0] = 77;
    bytes[15] = 99;
    if bytes[0] != 77 or bytes[15] != 99 {
        return process::ExitCode(7)!;
    }

    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::ExitCode(8)!; },
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
                return process::ExitCode(1)!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return process::ExitCode(2)!;
            } },
    }
    if allocator.isEmpty() or allocator.capacity() == 0 {
        return process::ExitCode(3)!;
    }
    allocator.deinitWithoutLeakCheck().?;
    if not allocator.isEmpty() {
        return process::ExitCode(4)!;
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
