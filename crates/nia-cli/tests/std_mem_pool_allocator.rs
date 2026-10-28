// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_mem_pool_allocator_reuses_slots_and_checks_layouts() {
    let root = temp_dir("emit_exe_std_mem_pool_allocator_reuses_slots_and_checks_layouts");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct Empty {}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut child = mem::PageAllocator::init();
    match mem::PoolAllocator[u64]::init(&mut child, 0) {
        !pool => { _ = pool; return process::ExitCode(22)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(23)!; },
    }
    match mem::PoolAllocator[u64]::init(&mut child, usize::MAX) {
        !pool => { _ = pool; return process::ExitCode(24)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(25)!; },
    }
    let mut pool = mem::PoolAllocator[u64]::init(&mut child, 3).?;
    let full = mem::Layout::of[u64]().?;
    let small = mem::Layout::init(1, 1).?;
    let medium = mem::Layout::init(4, 4).?;
    let oversized = mem::Layout::init(9, 1).?;
    let over_aligned = mem::Layout::init(1, 16).?;
    let empty = mem::Layout::init(0, 1).?;

    if pool.capacity() != 0 or pool.used() != 0 or pool.allocationCount() != 0 {
        return process::ExitCode(1)!;
    }
    let mut first = pool.alloc(full).?;
    let second = pool.alloc(small).?;
    let third = pool.alloc(medium).?;
    if first.ptr() as usize % 8 != 0
        or second.ptr() as usize % 8 != 0
        or third.ptr() as usize % 8 != 0
        or pool.capacity() != 24
        or pool.used() != 13
        or pool.allocationCount() != 3
    {
        return process::ExitCode(2)!;
    }

    if pool.resize(first, oversized) or pool.resize(first, empty) {
        return process::ExitCode(3)!;
    }
    if pool.resize(first, over_aligned) {
        return process::ExitCode(4)!;
    }
    let mut firstBytes = first.bytes();
    firstBytes[0] = 0x5a;
    if firstBytes[0] != 0x5a {
        return process::ExitCode(5)!;
    }

    pool.free(second).?;
    if pool.used() != 12 or pool.allocationCount() != 2 {
        return process::ExitCode(6)!;
    }
    match pool.free(second) {
        !ok => { _ = ok; return process::ExitCode(7)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(8)!; },
    }
    let replacement = pool.alloc(small).?;
    if replacement.ptr() as usize != second.ptr() as usize {
        return process::ExitCode(9)!;
    }
    let forgedLayout = mem::Block::init(first.ptr(), medium);
    match pool.free(forgedLayout) {
        !ok => { _ = ok; return process::ExitCode(34)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(35)!; },
    }
    let forgedAlignmentLayout = mem::Layout::init(8, 1).?;
    let forgedAlignment = mem::Block::init(first.ptr(), forgedAlignmentLayout);
    match pool.free(forgedAlignment) {
        !ok => { _ = ok; return process::ExitCode(38)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(39)!; },
    }
    let interiorPtr = ((first.ptr() as usize) + 1usize) as &mut u8;
    let forgedPointer = mem::Block::init(interiorPtr, small);
    match pool.free(forgedPointer) {
        !ok => { _ = ok; return process::ExitCode(36)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(37)!; },
    }

    first = match pool.remap(first, medium) {
        ?block => block,
        null => return process::ExitCode(10)!,
    };
    if first.size() != 4 or pool.used() != 9 {
        return process::ExitCode(11)!;
    }
    first = match pool.remap(first, full) {
        ?block => block,
        null => return process::ExitCode(12)!,
    };
    if first.size() != 8 or pool.used() != 13 {
        return process::ExitCode(13)!;
    }

    match pool.alloc(oversized) {
        !block => { _ = block; return process::ExitCode(14)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(15)!; },
    }
    match pool.alloc(over_aligned) {
        !block => { _ = block; return process::ExitCode(16)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(17)!; },
    }

    let fourth = pool.alloc(full).?;
    if pool.capacity() != 48 or pool.used() != 21 or pool.allocationCount() != 4 {
        return process::ExitCode(18)!;
    }
    let fifth = pool.alloc(full).?;
    let sixth = pool.alloc(full).?;
    if pool.capacity() != 48 or pool.used() != 37 or pool.allocationCount() != 6 {
        return process::ExitCode(32)!;
    }
    pool.free(fifth).?;
    let recycled = pool.alloc(full).?;
    if recycled.ptr() as usize != fifth.ptr() as usize
        or pool.capacity() != 48
        or pool.used() != 37
        or pool.allocationCount() != 6
    {
        return process::ExitCode(33)!;
    }
    let emptyBlock = pool.alloc(empty).?;
    if not emptyBlock.isEmpty() or pool.allocationCount() != 6 {
        return process::ExitCode(19)!;
    }
    let emptyRemapLayout = mem::Layout::init(0, 16).?;
    let remappedEmpty = match pool.remap(emptyBlock, emptyRemapLayout) {
        ?block => block,
        null => return process::ExitCode(40)!,
    };
    if not remappedEmpty.isEmpty() or remappedEmpty.align() != 16 {
        return process::ExitCode(41)!;
    }
    pool.free(remappedEmpty).?;

    pool.free(first).?;
    pool.free(replacement).?;
    pool.free(third).?;
    pool.free(fourth).?;
    pool.free(sixth).?;
    pool.free(recycled).?;
    if pool.used() != 0 or pool.allocationCount() != 0 or pool.capacity() != 48 {
        return process::ExitCode(20)!;
    }
    pool.deinit().?;
    if not pool.isEmpty() or pool.capacity() != 0 {
        return process::ExitCode(21)!;
    }

    let mut emptyPool = mem::PoolAllocator[Empty]::init(&mut child, 1).?;
    let emptyLayout = mem::Layout::of[Empty]().?;
    let emptyAllocation = emptyPool.alloc(emptyLayout).?;
    if not emptyAllocation.isEmpty()
        or emptyPool.capacity() != 0
        or emptyPool.used() != 0
        or emptyPool.allocationCount() != 0
    {
        return process::ExitCode(26)!;
    }
    emptyPool.free(emptyAllocation).?;
    match emptyPool.alloc(mem::Layout::init(1, 1).?) {
        !block => { _ = block; return process::ExitCode(27)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(28)!; },
    }
    emptyPool.deinit().?;

    let mut widePool = mem::PoolAllocator[u128]::init(&mut child, 2).?;
    let wideLayout = mem::Layout::of[u128]().?;
    let wideBlock = widePool.alloc(wideLayout).?;
    if wideBlock.ptr() as usize % wideLayout.align() != 0 {
        return process::ExitCode(42)!;
    }
    widePool.free(wideBlock).?;
    widePool.deinit().?;

    let mut tinyStorage: [u8; 8] = [0; 8];
    let mut tinyChild = mem::FixedBufferAllocator::init(&mut tinyStorage[..]);
    let mut tinyPool = mem::PoolAllocator[u64]::init(&mut tinyChild, 2).?;
    match tinyPool.alloc(full) {
        !block => { _ = block; return process::ExitCode(29)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(30)!; },
    }
    if tinyPool.capacity() != 0 or not tinyPool.isEmpty() {
        return process::ExitCode(31)!;
    }
    tinyPool.deinit().?;
    !()
}
"#,
    )
    .expect("write pool allocator test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile pool allocator behavior");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run pool allocator behavior");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_pool_allocator_retries_failed_cleanup_and_reports_leaks() {
    let root = temp_dir("emit_exe_std_mem_pool_allocator_retries_failed_cleanup_and_reports_leaks");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::process;

struct FailOnceAllocator {
    page: mem::PageAllocator,
    failures: usize,
}

extend FailOnceAllocator {
    fun init() FailOnceAllocator {
        Self { page: mem::PageAllocator::init(), failures: 1 }
    }
}

extend FailOnceAllocator : mem::Allocator {
    fun alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        self.page.alloc(layout)
    }

    fun free(&mut self, block: mem::Block) mem::Error!() {
        if self.failures != 0 {
            self.failures -= 1;
            return mem::Error::System!;
        }
        self.page.free(block)
    }
}

struct MalformedAllocator {
    failures: usize,
}

extend MalformedAllocator {
    fun init() MalformedAllocator {
        Self { failures: 1 }
    }
}

extend MalformedAllocator : mem::Allocator {
    fun alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        !mem::Block::init((usize::MAX - 7usize) as &mut u8, layout)
    }

    fun free(&mut self, block: mem::Block) mem::Error!() {
        _ = block;
        if self.failures != 0 {
            self.failures -= 1;
            return mem::Error::System!;
        }
        !()
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut child = FailOnceAllocator::init();
    let mut pool = mem::PoolAllocator[u32]::init(&mut child, 2).?;
    let layout = mem::Layout::of[u32]().?;
    _ = pool.alloc(layout).?;
    _ = pool.alloc(layout).?;
    let thirdBlock = pool.alloc(layout).?;

    match pool.deinit() {
        !ok => { _ = ok; return process::ExitCode(1)!; },
        mem::Error::System! => {},
        error! => { _ = error; return process::ExitCode(2)!; },
    }
    if pool.isEmpty()
        or pool.capacity() != 8
        or pool.used() != 4
        or pool.allocationCount() != 1
    {
        return process::ExitCode(3)!;
    }
    match pool.alloc(layout) {
        !newBlock => { _ = newBlock; return process::ExitCode(4)!; },
        mem::Error::Invalid! => {},
        error! => { _ = error; return process::ExitCode(5)!; },
    }
    pool.free(thirdBlock).?;
    if pool.used() != 0 or pool.allocationCount() != 0 or pool.capacity() != 8 {
        return process::ExitCode(16)!;
    }
    match pool.deinit() {
        !ok => { _ = ok; },
        mem::Error::Leak! => {},
        error! => { _ = error; return process::ExitCode(6)!; },
    }
    if not pool.isEmpty() {
        return process::ExitCode(7)!;
    }

    let mut leakPool = mem::PoolAllocator[u32]::init(&mut child, 2).?;
    _ = leakPool.alloc(layout).?;
    leakPool.deinitWithoutLeakCheck().?;
    if not leakPool.isEmpty() {
        return process::ExitCode(9)!;
    }
    leakPool.deinit().?;

    let mut malformedChild = MalformedAllocator::init();
    let mut malformedPool = mem::PoolAllocator[u32]::init(&mut malformedChild, 2).?;
    match malformedPool.alloc(layout) {
        !block => { _ = block; return process::ExitCode(10)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(11)!; },
    }
    if malformedPool.isEmpty() {
        return process::ExitCode(12)!;
    }
    match malformedPool.alloc(layout) {
        !block => { _ = block; return process::ExitCode(13)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(14)!; },
    }
    if not malformedPool.isEmpty() {
        return process::ExitCode(15)!;
    }
    malformedPool.deinit().?;
    !()
}
"#,
    )
    .expect("write pool cleanup test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile pool allocator cleanup behavior");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run pool allocator cleanup behavior");
    assert_eq!(status.code(), Some(0));
}
