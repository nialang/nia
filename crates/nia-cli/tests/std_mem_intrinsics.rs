// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

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
    let mut allocator = mem::PageAllocator::init();
    if !items = allocator.alloc_slice[i32](0) { if items.len() != 0 {
                return (2 as process::ExitCode)!;
            }
            if !ok = allocator.free_slice[i32](items) { _ = ok; } or error! { return (3 as process::ExitCode)!; } } or error! { return (1 as process::ExitCode)!; }
    !{}
}

fn check_allocator_preserves_zero_sized_slice_len() process::ExitCode!void {
    let mut allocator = mem::PageAllocator::init();
    if !items = allocator.alloc_slice[void](4) { if items.len() != 4 {
                return (2 as process::ExitCode)!;
            }
            if !ok = allocator.free_slice[void](items) { _ = ok; } or error! { return (3 as process::ExitCode)!; } } or error! { return (1 as process::ExitCode)!; }
    !{}
}

fn check_block_as_slice_handles_zero_sized_element_type() process::ExitCode!void {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    if !value = mem::Layout::array[void](8) { layout = value; } or error! { return (1 as process::ExitCode)!; }
    let mut block: mem::Block;
    if !value = allocator.alloc(layout) { block = value; } or error! { return (2 as process::ExitCode)!; }
    let mut items = block.as_slice[void]();
    if items.len() != 0 {
        return (3 as process::ExitCode)!;
    }
    if !ok = allocator.free(block) { _ = ok; } or error! { return (4 as process::ExitCode)!; }
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
    let mut left: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_forwards[i32](&mut left[0..3], &left[1..4]);
    let expected_left: [5]i32 = [2, 3, 4, 4, 5];
    if not mem::equal[i32](&left[..], &expected_left[..]) {
        return (1 as process::ExitCode)!;
    }

    let mut right: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_backwards[i32](&mut right[1..4], &right[0..3]);
    let expected_right: [5]i32 = [1, 1, 2, 3, 5];
    if not mem::equal[i32](&right[..], &expected_right[..]) {
        return (2 as process::ExitCode)!;
    }

    let mut exact_to: [3]u8 = [0, 0, 0];
    let exact_from: [3]u8 = [7, 8, 9];
    mem::copy_forwards[u8](&mut exact_to[..], &exact_from[..]);
    if not mem::equal[u8](&exact_to[..], &exact_from[..]) {
        return (3 as process::ExitCode)!;
    }

    let mut short_to: [2]u8 = [0, 0];
    let long_from: [4]u8 = [5, 6, 7, 8];
    mem::copy_forwards[u8](&mut short_to[..], &long_from[..]);
    let expected_short_to: [2]u8 = [5, 6];
    if not mem::equal[u8](&short_to[..], &expected_short_to[..]) {
        return (8 as process::ExitCode)!;
    }

    let mut short_backward: [2]u8 = [0, 0];
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

    let mut ints: [3]i32 = [0, 0, 0];
    let source_ints: [3]i32 = [7, 8, 9];
    @memcpy(&mut ints[..], &source_ints[..]);
    if ints[0] != 7 or ints[1] != 8 or ints[2] != 9 {
        return (1 as process::ExitCode)!;
    }

    let mut wide: [5]i32 = [0, 0, 0, 44, 55];
    let short: [3]i32 = [11, 22, 33];
    @memcpy(&mut wide[..], &short[..]);
    if wide[0] != 11 or wide[1] != 22 or wide[2] != 33 or wide[3] != 44 or wide[4] != 55 {
        return (4 as process::ExitCode)!;
    }

    let mut narrow: [4]u8 = [0, 0, 77, 88];
    let long: [4]u8 = [10, 20, 30, 40];
    @memcpy(&mut narrow[0..2], &long[..]);
    if narrow[0] != 10 or narrow[1] != 20 or narrow[2] != 77 or narrow[3] != 88 {
        return (5 as process::ExitCode)!;
    }

    let mut overlap: [5]u8 = [1, 2, 3, 4, 5];
    @memmove(&mut overlap[1..], &overlap[0..4]);
    if overlap[0] != 1 or overlap[1] != 1 or overlap[2] != 2 or overlap[3] != 3 or overlap[4] != 4 {
        return (2 as process::ExitCode)!;
    }

    let mut short_move: [4]u8 = [9, 8, 7, 6];
    @memmove(&mut short_move[0..2], &short_move[1..4]);
    if short_move[0] != 8 or short_move[1] != 7 or short_move[2] != 7 or short_move[3] != 6 {
        return (6 as process::ExitCode)!;
    }

    let mut bytes: [4]u8 = [1, 2, 3, 4];
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
    let mut dest: [2]u8 = [0; 2];
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
    let mut data: [3]i32 = [10, 20, 30];
    let mut total = 0;
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
