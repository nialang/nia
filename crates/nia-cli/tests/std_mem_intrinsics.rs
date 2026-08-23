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

fn check_allocator_preserves_empty_slice_len() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    match allocator.allocSlice[i32](0) {
        mut !allocation => { if allocation.len() != 0 or allocation.asSlice().len() != 0 {
                    return process::exit(2)!;
                }
                allocation.deinit(&mut allocator).exit().?; },
        error! => { return process::exit(1)!; },
    }
    !()
}

fn check_allocator_preserves_zero_sized_slice_len() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    match allocator.allocSlice[()](4) {
        mut !allocation => { if allocation.len() != 4 or allocation.asSlice().len() != 4 {
                    return process::exit(2)!;
                }
                allocation.deinit(&mut allocator).exit().?; },
        error! => { return process::exit(1)!; },
    }
    !()
}

fn check_block_as_slice_handles_zero_sized_element_type() process::ExitCode!() {
    let mut allocator = mem::PageAllocator::init();
    let mut layout: mem::Layout;
    match mem::Layout::array[()](8) {
        !value => { layout = value; },
        error! => { return process::exit(1)!; },
    }
    let mut block: mem::Block;
    match allocator.alloc(layout) {
        !value => { block = value; },
        error! => { return process::exit(2)!; },
    }
    let mut items = block.asSlice[()]();
    if items.len() != 0 {
        return process::exit(3)!;
    }
    match allocator.free(block) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
    }
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    check_allocator_preserves_empty_slice_len().?;
    check_allocator_preserves_zero_sized_slice_len().?;
    check_block_as_slice_handles_zero_sized_element_type().?;
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
fn emit_exe_memory_intrinsic_builtins() {
    let root = temp_dir("emit_exe_memory_intrinsic_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut ints: [i32; 3] = [0, 0, 0];
    let source_ints: [i32; 3] = [7, 8, 9];
    std::builtin::memcpy(&mut ints[..], &source_ints[..]);
    if ints[0] != 7 or ints[1] != 8 or ints[2] != 9 {
        return process::exit(1)!;
    }

    let mut wide: [i32; 5] = [0, 0, 0, 44, 55];
    let short: [i32; 3] = [11, 22, 33];
    std::builtin::memcpy(&mut wide[..], &short[..]);
    if wide[0] != 11 or wide[1] != 22 or wide[2] != 33 or wide[3] != 44 or wide[4] != 55 {
        return process::exit(4)!;
    }

    let mut narrow: [u8; 4] = [0, 0, 77, 88];
    let long: [u8; 4] = [10, 20, 30, 40];
    std::builtin::memcpy(&mut narrow[0..2], &long[..]);
    if narrow[0] != 10 or narrow[1] != 20 or narrow[2] != 77 or narrow[3] != 88 {
        return process::exit(5)!;
    }

    let mut overlap: [u8; 5] = [1, 2, 3, 4, 5];
    std::builtin::memmove(&mut overlap[1..], &overlap[0..4]);
    if overlap[0] != 1 or overlap[1] != 1 or overlap[2] != 2 or overlap[3] != 3 or overlap[4] != 4 {
        return process::exit(2)!;
    }

    let mut short_move: [u8; 4] = [9, 8, 7, 6];
    std::builtin::memmove(&mut short_move[0..2], &short_move[1..4]);
    if short_move[0] != 8 or short_move[1] != 7 or short_move[2] != 7 or short_move[3] != 6 {
        return process::exit(6)!;
    }

    let mut bytes: [u8; 4] = [1, 2, 3, 4];
    std::builtin::memset(&mut bytes[1..3], 9);
    if bytes[0] != 1 or bytes[1] != 9 or bytes[2] != 9 or bytes[3] != 4 {
        return process::exit(3)!;
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
fn emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals() {
    let root = temp_dir("emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals");
    let main = root.join("main.nia");
    let helper = root.join("helper.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub fn copy_prefix[T](to: &mut [T], from: &[T]) ()
where T: Sized
{
    std::builtin::memcpy(to, from);
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        &main,
        r#"
using helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut dest: [u8; 2] = [0; 2];
    let source: [u8; 2] = [b'a', b'b'];
    helper::copy_prefix[u8](&mut dest[..], &source[..]);
    if dest[0] != b'a' or dest[1] != b'b' {
        return process::exit(1)!;
    }
    !()
}
"#,
    )
    .expect("write main source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
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
        Self { ptr, len, index: 0 }
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
            let item = (self.ptr as usize + self.index * std::builtin::size[T]()) as &T;
            self.index += 1;
            ?item
        }
    }
}

extend[T] [T]
where T: Sized
{
    pub fn iter_custom(&self) SliceIter[T] {
        SliceIter[T]::from_raw_parts(self.ptr(), self.len())
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut data: [i32; 3] = [10, 20, 30];
    let mut total = 0;
    for &value in (&data[..]).iter_custom() {
        total += value;
    }
    if total != 60 {
        return process::exit(1)!;
    }
    !()
}
"#,
    )
    .expect("write main source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
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
