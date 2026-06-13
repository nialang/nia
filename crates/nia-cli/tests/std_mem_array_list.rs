// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

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
    if let !value = std::ArrayList[i32]::init_capacity(page, 3) { exact = value; } else error! { return (1 as process::ExitCode)!; }
    if exact.len() != 0 or exact.capacity() != 3 {
        return (2 as process::ExitCode)!;
    }
    if let !ok = exact.deinit(page) { _ = ok; } else error! { return (3 as process::ExitCode)!; }

    var ops = std::ArrayList[i32]::init();
    if let !ok = ops.push(page, 1) { _ = ok; } else error! { return (26 as process::ExitCode)!; }
    if let !ok = ops.push(page, 3) { _ = ok; } else error! { return (27 as process::ExitCode)!; }
    if let !ok = ops.insert(page, 1, 2) { _ = ok; } else error! { return (28 as process::ExitCode)!; }
    let inserted_tail: [2]i32 = [4, 5];
    if let !ok = ops.insert_slice(page, 3, &inserted_tail[..]) { _ = ok; } else error! { return (29 as process::ExitCode)!; }
    let expected_ops: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ops[..]) {
        return (30 as process::ExitCode)!;
    }
    if let ?value = ops.ordered_remove(1) { if value != 2 {
                return (31 as process::ExitCode)!;
            } } else null { return (32 as process::ExitCode)!; }
    let expected_ordered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ordered[..]) {
        return (33 as process::ExitCode)!;
    }
    if let ?value = ops.swap_remove(0) { if value != 1 {
                return (34 as process::ExitCode)!;
            } } else null { return (35 as process::ExitCode)!; }
    let expected_swap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.as_slice(), &expected_swap[..]) {
        return (36 as process::ExitCode)!;
    }
    if let !ok = ops.deinit(page) { _ = ok; } else error! { return (37 as process::ExitCode)!; }

    var alias = std::ArrayList[i32]::init();
    if let !ok = alias.reserve_exact(page, 2) { _ = ok; } else error! { return (38 as process::ExitCode)!; }
    if let !ok = alias.push(page, 1) { _ = ok; } else error! { return (39 as process::ExitCode)!; }
    if let !ok = alias.push(page, 2) { _ = ok; } else error! { return (40 as process::ExitCode)!; }
    if let !ok = alias.append_slice(page, alias.as_slice()) { _ = ok; } else error! { return (41 as process::ExitCode)!; }
    let expected_alias_append: [4]i32 = [1, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_append[..]) {
        return (42 as process::ExitCode)!;
    }
    if let !ok = alias.insert_slice(page, 1, alias.as_slice()) { _ = ok; } else error! { return (43 as process::ExitCode)!; }
    let expected_alias_insert: [8]i32 = [1, 1, 2, 1, 2, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_insert[..]) {
        return (44 as process::ExitCode)!;
    }
    if let !ok = alias.deinit(page) { _ = ok; } else error! { return (45 as process::ExitCode)!; }

    var list = std::ArrayList[i32]::init();
    if list.len() != 0 or not list.is_empty() {
        return (4 as process::ExitCode)!;
    }
    if let !ok = list.reserve_exact(page, 2) { _ = ok; } else error! { return (5 as process::ExitCode)!; }
    if list.capacity() != 2 {
        return (6 as process::ExitCode)!;
    }
    if let !ok = list.reserve(page, 3) { _ = ok; } else error! { return (7 as process::ExitCode)!; }
    if list.capacity() < 5 {
        return (8 as process::ExitCode)!;
    }
    var index = 0;
    while index < 6 {
        if let !ok = list.push(page, index * 10) { _ = ok; } else error! { return (9 as process::ExitCode)!; }
        index += 1;
    }
    if list.len() != 6 or list.capacity() < 6 {
        return (10 as process::ExitCode)!;
    }
    let items = list.as_slice();
    if items[0] != 0 or items[1] != 10 or items[5] != 50 {
        return (11 as process::ExitCode)!;
    }
    if let ?value = list.first() { if value.* != 0 {
                return (64 as process::ExitCode)!;
            } } else null { return (65 as process::ExitCode)!; }
    if let ?value = list.last() { if value.* != 50 {
                return (66 as process::ExitCode)!;
            } } else null { return (67 as process::ExitCode)!; }
    if let ?value = list.get(3) { if value.* != 30 {
                return (68 as process::ExitCode)!;
            } } else null { return (69 as process::ExitCode)!; }
    if let ?value = list.get(6) { _ = value;
            return (70 as process::ExitCode)!; } else null { }
    if var ?value = list.get_mut(4) { value.* = 44; } else null { return (71 as process::ExitCode)!; }
    if var ?value = list.last_mut() { value.* = 55; } else null { return (72 as process::ExitCode)!; }
    let expected_after_accessors: [6]i32 = [0, 10, 20, 30, 44, 55];
    if not mem::equal[i32](list.as_slice(), &expected_after_accessors[..]) {
        return (73 as process::ExitCode)!;
    }

    let more: [3]i32 = [60, 70, 80];
    if let !ok = list.append_slice(page, &more[..]) { _ = ok; } else error! { return (12 as process::ExitCode)!; }
    if list.len() != 9 or list.as_slice()[8] != 80 {
        return (13 as process::ExitCode)!;
    }

    if var !slot = list.add_one(page) { slot.* = 90; } else error! { return (14 as process::ExitCode)!; }
    if list.len() != 10 or list.as_slice()[9] != 90 {
        return (15 as process::ExitCode)!;
    }

    if var !slots = list.add_many_as_slice(page, 2) { slots[0] = 100;
            slots[1] = 110; } else error! { return (16 as process::ExitCode)!; }
    if list.len() != 12 or list.as_slice()[11] != 110 {
        return (17 as process::ExitCode)!;
    }

    if var !slots = list.add_many_at(page, 2, 2) { slots[0] = 21;
            slots[1] = 22; } else error! { return (46 as process::ExitCode)!; }
    if list.len() != 14 or list.as_slice()[2] != 21 or list.as_slice()[3] != 22 or list.as_slice()[4] != 20 {
        return (47 as process::ExitCode)!;
    }

    list.append_assume_capacity(120);
    if list.len() != 15 or list.as_slice()[14] != 120 {
        return (48 as process::ExitCode)!;
    }

    if let !ok = list.resize(page, 18) { _ = ok; } else error! { return (49 as process::ExitCode)!; }
    if list.len() != 18 {
        return (50 as process::ExitCode)!;
    }
    var unused = list.unused_capacity_slice();
    if unused.len() < 2 {
        return (51 as process::ExitCode)!;
    }
    unused[0] = 180;
    unused[1] = 190;
    if let !slots = list.add_many_as_slice(page, 2) { if slots[0] != 180 or slots[1] != 190 {
                return (52 as process::ExitCode)!;
            } } else error! { return (53 as process::ExitCode)!; }
    if list.len() != 20 {
        return (54 as process::ExitCode)!;
    }

    if let !ok = list.resize(page, 12) { _ = ok; } else error! { return (55 as process::ExitCode)!; }
    if list.len() != 12 {
        return (56 as process::ExitCode)!;
    }

    let before_shrink_capacity = list.capacity();
    if let !ok = list.shrink_to_len(page) { _ = ok; } else error! { return (57 as process::ExitCode)!; }
    if list.len() != 12 or list.capacity() > before_shrink_capacity or list.capacity() < list.len() {
        return (58 as process::ExitCode)!;
    }

    if let !ok = list.reserve_exact(page, 4) { _ = ok; } else error! { return (59 as process::ExitCode)!; }
    list.expand_to_capacity();
    if list.len() != list.capacity() {
        return (60 as process::ExitCode)!;
    }

    if let !ok = list.shrink_and_free(page, 10) { _ = ok; } else error! { return (61 as process::ExitCode)!; }
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

    if let !ok = list.reserve_exact(page, 2) { _ = ok; } else error! { return (74 as process::ExitCode)!; }
    let tail: [2]i32 = [100, 110];
    list.append_slice_assume_capacity(&tail[..]);
    if list.len() != 12 or list.as_slice()[10] != 100 or list.as_slice()[11] != 110 {
        return (19 as process::ExitCode)!;
    }

    if let ?value = list.pop() { if value != 110 {
                return (20 as process::ExitCode)!;
            } } else null { return (21 as process::ExitCode)!; }
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
    if let !ok = list.clear_and_free(page) { _ = ok; } else error! { return (25 as process::ExitCode)!; }
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
    if let !ok = list.push(page, 10) { _ = ok; } else error! { return (1 as process::ExitCode)!; }
    if let !ok = list.push(page, 20) { _ = ok; } else error! { return (2 as process::ExitCode)!; }
    if let !ok = list.shrink_to_capacity(page, 0) { _ = ok; } else error! { return (3 as process::ExitCode)!; }
    if list.len() != 0 or list.capacity() != 0 {
        return (4 as process::ExitCode)!;
    }

    if let !ok = list.push(page, 30) { _ = ok; } else error! { return (5 as process::ExitCode)!; }
    if let !ok = list.push(page, 40) { _ = ok; } else error! { return (6 as process::ExitCode)!; }
    let expected: [2]i32 = [30, 40];
    if not mem::equal[i32](list.as_slice(), &expected[..]) {
        return (7 as process::ExitCode)!;
    }
    if let !ok = list.deinit(page) { _ = ok; } else error! { return (8 as process::ExitCode)!; }
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
    if let !ok = source.push(page, 1) { _ = ok; } else error! { return (1 as process::ExitCode)!; }
    if let !ok = source.push(page, 2) { _ = ok; } else error! { return (2 as process::ExitCode)!; }

    var cloned: std::ArrayList[i32];
    if let !value = source.clone(page) { cloned = value; } else error! { return (3 as process::ExitCode)!; }
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
    if let !value = source.into_owned_slice(page) { owned = value; } else error! { return (6 as process::ExitCode)!; }
    if source.len() != 0 or source.capacity() != 0 {
        return (7 as process::ExitCode)!;
    }
    if not mem::equal[i32](owned, &expected_source[..]) {
        return (8 as process::ExitCode)!;
    }
    if let !ok = page.free_slice[i32](owned) { _ = ok; } else error! { return (9 as process::ExitCode)!; }

    var external: &mut [i32];
    if let !items = page.alloc_slice[i32](3) { external = items; } else error! { return (10 as process::ExitCode)!; }
    external[0] = 4;
    external[1] = 5;
    external[2] = 6;
    var adopted = std::ArrayList[i32]::from_owned_slice(external);
    let expected_adopted: [3]i32 = [4, 5, 6];
    if adopted.capacity() != 3 or not mem::equal[i32](adopted.as_slice(), &expected_adopted[..]) {
        return (11 as process::ExitCode)!;
    }
    if let !ok = adopted.deinit(page) { _ = ok; } else error! { return (12 as process::ExitCode)!; }
    if let !ok = cloned.deinit(page) { _ = ok; } else error! { return (13 as process::ExitCode)!; }
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
    if let !ok = list.reserve(page, 4) { _ = ok; } else error! { return (1 as process::ExitCode)!; }
    if list.capacity() != usize::MAX {
        return (2 as process::ExitCode)!;
    }
    if let !ok = list.push(page, {}) { _ = ok; } else error! { return (3 as process::ExitCode)!; }
    if let !ok = list.resize(page, 16) { _ = ok; } else error! { return (4 as process::ExitCode)!; }
    if list.len() != 16 or list.capacity() != usize::MAX {
        return (5 as process::ExitCode)!;
    }
    if let !ok = list.shrink_and_free(page, 3) { _ = ok; } else error! { return (6 as process::ExitCode)!; }
    if list.len() != 3 or list.capacity() != usize::MAX {
        return (7 as process::ExitCode)!;
    }
    if let !ok = list.deinit(page) { _ = ok; } else error! { return (8 as process::ExitCode)!; }
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
fn emit_exe_std_array_list_range_operations_and_owned_copy() {
    let root = temp_dir("emit_exe_std_array_list_range_operations_and_owned_copy");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;

fn expect_invalid(result: mem::Error!void) process::ExitCode!void {
    if let !ok = result { _ = ok;
            return (90 as process::ExitCode)!; } else err! { if err as i32 != mem::Error::Invalid as i32 {
                return (91 as process::ExitCode)!;
            } }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;

    var list = std::ArrayList[i32]::init();
    defer list.deinit(page).exit().?;

    let initial: [8]i32 = [0, 1, 2, 3, 4, 5, 6, 7];
    list.append_slice(page, &initial[..]).exit().?;

    list.remove_range(2, 3).exit().?;
    let after_remove: [5]i32 = [0, 1, 5, 6, 7];
    if not mem::equal[i32](list.as_slice(), &after_remove[..]) {
        return (1 as process::ExitCode)!;
    }

    let same_len: [2]i32 = [10, 11];
    list.replace_range(page, 1, 2, &same_len[..]).exit().?;
    let after_same_len: [5]i32 = [0, 10, 11, 6, 7];
    if not mem::equal[i32](list.as_slice(), &after_same_len[..]) {
        return (2 as process::ExitCode)!;
    }

    let smaller: [1]i32 = [20];
    list.replace_range(page, 2, 2, &smaller[..]).exit().?;
    let after_smaller: [4]i32 = [0, 10, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_smaller[..]) {
        return (3 as process::ExitCode)!;
    }

    let larger: [4]i32 = [30, 31, 32, 33];
    list.replace_range(page, 1, 1, &larger[..]).exit().?;
    let after_larger: [7]i32 = [0, 30, 31, 32, 33, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_larger[..]) {
        return (4 as process::ExitCode)!;
    }

    list.replace_range(page, 2, 3, list.as_slice()).exit().?;
    let after_alias_replace: [11]i32 = [0, 30, 0, 30, 31, 32, 33, 20, 7, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_alias_replace[..]) {
        return (5 as process::ExitCode)!;
    }

    list.truncate(6);
    let after_truncate: [6]i32 = [0, 30, 0, 30, 31, 32];
    if not mem::equal[i32](list.as_slice(), &after_truncate[..]) {
        return (6 as process::ExitCode)!;
    }
    list.truncate(99);
    if list.len() != 6 {
        return (7 as process::ExitCode)!;
    }

    var owned = list.to_owned_slice(page).exit().?;
    if not mem::equal[i32](owned, list.as_slice()) {
        return (8 as process::ExitCode)!;
    }
    owned[0] = 1234;
    if list.as_slice()[0] == 1234 {
        return (9 as process::ExitCode)!;
    }
    page.free_slice[i32](owned).exit().?;

    expect_invalid(list.remove_range(7, 1)).?;
    expect_invalid(list.remove_range(5, 2)).?;
    let invalid_values: [1]i32 = [99];
    expect_invalid(list.replace_range(page, 7, 0, &invalid_values[..])).?;
    expect_invalid(list.replace_range(page, 5, 2, &invalid_values[..])).?;
    if not mem::equal[i32](list.as_slice(), &after_truncate[..]) {
        return (10 as process::ExitCode)!;
    }

    var zero = std::ArrayList[void]::init();
    zero.resize(page, 4).exit().?;
    var zero_owned = zero.to_owned_slice(page).exit().?;
    if zero_owned.len() != 4 {
        return (11 as process::ExitCode)!;
    }
    zero.deinit(page).exit().?;
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
