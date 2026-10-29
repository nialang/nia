// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;
use nia_test_support::{CommandExt, test_dir};

#[test]
fn emit_exe_std_mem_layout_composes_regions_and_preserves_failed_state() {
    let root = test_dir("emit_exe_std_mem_layout_composes_regions_and_preserves_failed_state");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::mem::{Error, Layout, LayoutBuilder};
using std::process;
using std::slice;

struct Empty {}

fun outOfMemory(result: Error!usize) bool {
    if result is error! { error == Error::OutOfMemory } else { false }
}

fun checkPair(baseSize: usize, baseAlign: usize, nextSize: usize, nextAlign: usize) Error!bool {
    let base = Layout::init(baseSize, baseAlign).?;
    let next = Layout::init(nextSize, nextAlign).?;
    let mut builder = LayoutBuilder::init(base);
    let offset = builder.append(next).?;
    let expectedOffset = ((baseSize + nextAlign - 1) / nextAlign) * nextAlign;
    let expectedAlign = if baseAlign > nextAlign { baseAlign } else { nextAlign };
    if offset != expectedOffset or builder.layout().size() != expectedOffset + nextSize
        or builder.layout().align() != expectedAlign
    {
        return !false;
    }
    let second = builder.append(Layout::init(3, 4).?).?;
    let expectedSecond = ((expectedOffset + nextSize + 3) / 4) * 4;
    !(second == expectedSecond and builder.layout().size() == expectedSecond + 3)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let alignments: [usize; 6] = [1, 2, 4, 8, 16, 32];
    for baseSize in 0usize..18usize {
        for &baseAlign in &alignments[..] {
            for nextSize in 0usize..18usize {
                for &nextAlign in &alignments[..] {
                    if not checkPair(baseSize, baseAlign, nextSize, nextAlign).? {
                        return process::ExitCode(2)!;
                    }
                }
            }
        }
    }

    let mut typed = LayoutBuilder::of[u8]().?;
    let words = typed.appendArray[u32](3).?;
    let wide = typed.appendArray[u128](2).?;
    let layout = typed.layout();
    if words % std::builtin::align[u32]() != 0 or wide % std::builtin::align[u128]() != 0
        or wide < words + std::builtin::size[u32]() * 3
        or layout.size() != wide + std::builtin::size[u128]() * 2
        or layout.align() != std::builtin::align[u128]()
    {
        return process::ExitCode(3)!;
    }
    if not outOfMemory(typed.appendArray[u16](usize::MAX))
        or typed.layout().size() != layout.size() or typed.layout().align() != layout.align()
    {
        return process::ExitCode(4)!;
    }
    if typed.appendArray[Empty](usize::MAX).? != layout.size()
        or typed.layout().size() != layout.size()
    {
        return process::ExitCode(5)!;
    }

    let mut huge = LayoutBuilder::init(Layout::init(usize::MAX, 1).?);
    if not outOfMemory(huge.append(Layout::init(1, 1).?))
        or huge.layout().size() != usize::MAX or huge.layout().align() != 1
    {
        return process::ExitCode(6)!;
    }
    if not outOfMemory(huge.append(Layout::init(0, 2).?))
        or huge.layout().size() != usize::MAX or huge.layout().align() != 1
    {
        return process::ExitCode(7)!;
    }
    if huge.appendArray[Empty](usize::MAX).? != usize::MAX {
        return process::ExitCode(8)!;
    }

    let mut empty = LayoutBuilder::of[Empty]().?;
    if empty.append(Layout::init(0, 32).?).? != 0
        or not empty.layout().isEmpty() or empty.layout().align() != 32
    {
        return process::ExitCode(9)!;
    }
    let mut aligned = LayoutBuilder::of[u8]().?;
    if aligned.appendArray[u128](0).? != std::builtin::align[u128]()
        or aligned.layout().size() != std::builtin::align[u128]()
        or aligned.layout().align() != std::builtin::align[u128]()
    {
        return process::ExitCode(10)!;
    }
    let invalidAlignments: [usize; 3] = [0, 3, 6];
    for &alignment in &invalidAlignments[..] {
        if Layout::init(0, alignment) is error! {
            if error != Error::InvalidAlignment { return process::ExitCode(11)!; }
        } else {
            return process::ExitCode(12)!;
        }
    }

    let mut allocator = std::mem::PageAllocator::init();
    let block = allocator.alloc(layout).?;
    defer allocator.free(block).?;
    let mut firstWord = ((block.ptr() as usize) + words) as &mut u32;
    let mut firstWide = ((block.ptr() as usize) + wide) as &mut u128;
    firstWord.* = 17;
    firstWide.* = 29;
    if firstWord.* != 17 or firstWide.* != 29 { return process::ExitCode(13)!; }
    !()
}
"#,
    )
    .expect("write layout composition source");
    let emitted = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit layout composition probe");
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run layout composition probe");
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
