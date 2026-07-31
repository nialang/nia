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
    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut exact: std::ArrayList[i32];
    switch std::ArrayList[i32]::init_capacity(page, 3) {
        !value => { exact = value; },
        error! => { return process::exit(1)!; },
    }
    if exact.len() != 0 or exact.capacity() != 3 {
        return process::exit(2)!;
    }
    switch exact.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }

    let mut ops = std::ArrayList[i32]::init();
    switch ops.push(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(26)!; },
    }
    switch ops.push(page, 3) {
        !ok => { _ = ok; },
        error! => { return process::exit(27)!; },
    }
    switch ops.insert(page, 1, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(28)!; },
    }
    let inserted_tail: [2]i32 = [4, 5];
    switch ops.insert_slice(page, 3, &inserted_tail[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(29)!; },
    }
    let expected_ops: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ops[..]) {
        return process::exit(30)!;
    }
    switch ops.ordered_remove(1) {
        ?value => { if value != 2 {
                    return process::exit(31)!;
                } },
        null => { return process::exit(32)!; },
    }
    let expected_ordered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ordered[..]) {
        return process::exit(33)!;
    }
    switch ops.swap_remove(0) {
        ?value => { if value != 1 {
                    return process::exit(34)!;
                } },
        null => { return process::exit(35)!; },
    }
    let expected_swap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.as_slice(), &expected_swap[..]) {
        return process::exit(36)!;
    }
    switch ops.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(37)!; },
    }

    let mut alias = std::ArrayList[i32]::init();
    switch alias.reserve_exact(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(38)!; },
    }
    switch alias.push(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(39)!; },
    }
    switch alias.push(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(40)!; },
    }
    switch alias.append_slice(page, alias.as_slice()) {
        !ok => { _ = ok; },
        error! => { return process::exit(41)!; },
    }
    let expected_alias_append: [4]i32 = [1, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_append[..]) {
        return process::exit(42)!;
    }
    switch alias.insert_slice(page, 1, alias.as_slice()) {
        !ok => { _ = ok; },
        error! => { return process::exit(43)!; },
    }
    let expected_alias_insert: [8]i32 = [1, 1, 2, 1, 2, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_insert[..]) {
        return process::exit(44)!;
    }
    switch alias.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(45)!; },
    }

    let mut iter_list = std::ArrayList[i32]::init();
    switch iter_list.push(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(75)!; },
    }
    switch iter_list.push(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(76)!; },
    }
    switch iter_list.push(page, 3) {
        !ok => { _ = ok; },
        error! => { return process::exit(77)!; },
    }
    for value in iter_list.iter_mut() {
        value.* = value.* * 2;
    }
    for value in iter_list.iter_mut().rev().take(2usize) {
        value.* += 1;
    }
    let mut iter_sum = 0;
    for &value in iter_list {
        iter_sum += value;
    }
    if iter_sum != 14 {
        return process::exit(80)!;
    }
    let expected_iter_mut: [3]i32 = [2, 5, 7];
    if not mem::equal[i32](iter_list.as_slice(), &expected_iter_mut[..]) {
        return process::exit(78)!;
    }
    switch iter_list.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(79)!; },
    }

    let mut list = std::ArrayList[i32]::init();
    if list.len() != 0 or not list.is_empty() {
        return process::exit(4)!;
    }
    switch list.reserve_exact(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(5)!; },
    }
    if list.capacity() != 2 {
        return process::exit(6)!;
    }
    switch list.reserve(page, 3) {
        !ok => { _ = ok; },
        error! => { return process::exit(7)!; },
    }
    if list.capacity() < 5 {
        return process::exit(8)!;
    }
    let mut index = 0;
    while index < 6 {
        switch list.push(page, index * 10) {
            !ok => { _ = ok; },
            error! => { return process::exit(9)!; },
        }
        index += 1;
    }
    if list.len() != 6 or list.capacity() < 6 {
        return process::exit(10)!;
    }
    let items = list.as_slice();
    if items[0] != 0 or items[1] != 10 or items[5] != 50 {
        return process::exit(11)!;
    }
    switch list.first() {
        ?value => { if value.* != 0 {
                    return process::exit(64)!;
                } },
        null => { return process::exit(65)!; },
    }
    switch list.last() {
        ?value => { if value.* != 50 {
                    return process::exit(66)!;
                } },
        null => { return process::exit(67)!; },
    }
    switch list.get(3) {
        ?value => { if value.* != 30 {
                    return process::exit(68)!;
                } },
        null => { return process::exit(69)!; },
    }
    switch list.get(6) {
        ?value => { _ = value;
                return process::exit(70)!; },
        null => { },
    }
    switch list.get_mut(4) {
        mut ?value => { value.* = 44; },
        null => { return process::exit(71)!; },
    }
    switch list.last_mut() {
        mut ?value => { value.* = 55; },
        null => { return process::exit(72)!; },
    }
    let expected_after_accessors: [6]i32 = [0, 10, 20, 30, 44, 55];
    if not mem::equal[i32](list.as_slice(), &expected_after_accessors[..]) {
        return process::exit(73)!;
    }

    let more: [3]i32 = [60, 70, 80];
    switch list.append_slice(page, &more[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(12)!; },
    }
    if list.len() != 9 or list.as_slice()[8] != 80 {
        return process::exit(13)!;
    }

    switch list.add_one(page) {
        mut !slot => { slot.* = 90; },
        error! => { return process::exit(14)!; },
    }
    if list.len() != 10 or list.as_slice()[9] != 90 {
        return process::exit(15)!;
    }

    switch list.add_many_as_slice(page, 2) {
        mut !slots => { slots[0] = 100;
                slots[1] = 110; },
        error! => { return process::exit(16)!; },
    }
    if list.len() != 12 or list.as_slice()[11] != 110 {
        return process::exit(17)!;
    }

    switch list.add_many_at(page, 2, 2) {
        mut !slots => { slots[0] = 21;
                slots[1] = 22; },
        error! => { return process::exit(46)!; },
    }
    if list.len() != 14 or list.as_slice()[2] != 21 or list.as_slice()[3] != 22 or list.as_slice()[4] != 20 {
        return process::exit(47)!;
    }

    list.append_assume_capacity(120);
    if list.len() != 15 or list.as_slice()[14] != 120 {
        return process::exit(48)!;
    }

    switch list.resize(page, 18) {
        !ok => { _ = ok; },
        error! => { return process::exit(49)!; },
    }
    if list.len() != 18 {
        return process::exit(50)!;
    }
    let mut unused = list.unused_capacity_slice();
    if unused.len() < 2 {
        return process::exit(51)!;
    }
    unused[0] = 180;
    unused[1] = 190;
    switch list.add_many_as_slice(page, 2) {
        !slots => { if slots[0] != 180 or slots[1] != 190 {
                    return process::exit(52)!;
                } },
        error! => { return process::exit(53)!; },
    }
    if list.len() != 20 {
        return process::exit(54)!;
    }

    switch list.resize(page, 12) {
        !ok => { _ = ok; },
        error! => { return process::exit(55)!; },
    }
    if list.len() != 12 {
        return process::exit(56)!;
    }

    let before_shrink_capacity = list.capacity();
    switch list.shrink_to_len(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(57)!; },
    }
    if list.len() != 12 or list.capacity() > before_shrink_capacity or list.capacity() < list.len() {
        return process::exit(58)!;
    }

    switch list.reserve_exact(page, 4) {
        !ok => { _ = ok; },
        error! => { return process::exit(59)!; },
    }
    list.expand_to_capacity();
    if list.len() != list.capacity() {
        return process::exit(60)!;
    }

    switch list.shrink_and_free(page, 10) {
        !ok => { _ = ok; },
        error! => { return process::exit(61)!; },
    }
    if list.len() != 10 or list.capacity() < 10 {
        return process::exit(62)!;
    }

    let allocated = list.allocated_slice();
    if allocated.len() != list.capacity() {
        return process::exit(63)!;
    }

    let retained_capacity = list.capacity();
    list.shrink_retaining_capacity(10);
    if list.len() != 10 or list.capacity() != retained_capacity {
        return process::exit(18)!;
    }

    switch list.reserve_exact(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(74)!; },
    }
    let tail: [2]i32 = [100, 110];
    list.append_slice_assume_capacity(&tail[..]);
    if list.len() != 12 or list.as_slice()[10] != 100 or list.as_slice()[11] != 110 {
        return process::exit(19)!;
    }

    switch list.pop() {
        ?value => { if value != 110 {
                    return process::exit(20)!;
                } },
        null => { return process::exit(21)!; },
    }
    if list.len() != 11 {
        return process::exit(22)!;
    }
    let mut mutable_items = list.as_mut_slice();
    mutable_items[2] = 77;
    if list.as_slice()[2] != 77 {
        return process::exit(23)!;
    }
    list.clear_retaining_capacity();
    if not list.is_empty() {
        return process::exit(24)!;
    }
    switch list.clear_and_free(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(25)!; },
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
    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut list = std::ArrayList[i32]::init();
    switch list.push(page, 10) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    switch list.push(page, 20) {
        !ok => { _ = ok; },
        error! => { return process::exit(2)!; },
    }
    switch list.shrink_to_capacity(page, 0) {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    if list.len() != 0 or list.capacity() != 0 {
        return process::exit(4)!;
    }

    switch list.push(page, 30) {
        !ok => { _ = ok; },
        error! => { return process::exit(5)!; },
    }
    switch list.push(page, 40) {
        !ok => { _ = ok; },
        error! => { return process::exit(6)!; },
    }
    let expected: [2]i32 = [30, 40];
    if not mem::equal[i32](list.as_slice(), &expected[..]) {
        return process::exit(7)!;
    }
    switch list.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(8)!; },
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
    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;

    let mut source = std::ArrayList[i32]::init();
    switch source.push(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    switch source.push(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(2)!; },
    }

    let mut cloned: std::ArrayList[i32];
    switch source.clone(page) {
        !value => { cloned = value; },
        error! => { return process::exit(3)!; },
    }
    let mut source_items = source.as_mut_slice();
    source_items[0] = 9;
    let expected_source: [2]i32 = [9, 2];
    let expected_clone: [2]i32 = [1, 2];
    if not mem::equal[i32](source.as_slice(), &expected_source[..]) {
        return process::exit(4)!;
    }
    if not mem::equal[i32](cloned.as_slice(), &expected_clone[..]) {
        return process::exit(5)!;
    }

    let mut owned: &mut [i32];
    switch source.into_owned_slice(page) {
        !value => { owned = value; },
        error! => { return process::exit(6)!; },
    }
    if source.len() != 0 or source.capacity() != 0 {
        return process::exit(7)!;
    }
    if not mem::equal[i32](owned, &expected_source[..]) {
        return process::exit(8)!;
    }
    switch page.free_slice[i32](owned) {
        !ok => { _ = ok; },
        error! => { return process::exit(9)!; },
    }

    let mut external: &mut [i32];
    switch page.alloc_slice[i32](3) {
        !items => { external = items; },
        error! => { return process::exit(10)!; },
    }
    external[0] = 4;
    external[1] = 5;
    external[2] = 6;
    let mut adopted = std::ArrayList[i32]::from_owned_slice(external);
    let expected_adopted: [3]i32 = [4, 5, 6];
    if adopted.capacity() != 3 or not mem::equal[i32](adopted.as_slice(), &expected_adopted[..]) {
        return process::exit(11)!;
    }
    switch adopted.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(12)!; },
    }
    switch cloned.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(13)!; },
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
    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut list = std::ArrayList[Marker]::init();
    switch list.reserve(page, 4) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if list.capacity() != usize::MAX {
        return process::exit(2)!;
    }
    switch list.push(page, {}) {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    switch list.resize(page, 16) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
    }
    if list.len() != 16 or list.capacity() != usize::MAX {
        return process::exit(5)!;
    }
    switch list.shrink_and_free(page, 3) {
        !ok => { _ = ok; },
        error! => { return process::exit(6)!; },
    }
    if list.len() != 3 or list.capacity() != usize::MAX {
        return process::exit(7)!;
    }
    switch list.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(8)!; },
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
    switch result {
        !ok => { _ = ok;
                return process::exit(90)!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                    return process::exit(91)!;
                } },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;

    let mut list = std::ArrayList[i32]::init();
    defer list.deinit(page).exit().?;

    let initial: [8]i32 = [0, 1, 2, 3, 4, 5, 6, 7];
    list.append_slice(page, &initial[..]).exit().?;

    list.remove_range(2, 3).exit().?;
    let after_remove: [5]i32 = [0, 1, 5, 6, 7];
    if not mem::equal[i32](list.as_slice(), &after_remove[..]) {
        return process::exit(1)!;
    }

    let same_len: [2]i32 = [10, 11];
    list.replace_range(page, 1, 2, &same_len[..]).exit().?;
    let after_same_len: [5]i32 = [0, 10, 11, 6, 7];
    if not mem::equal[i32](list.as_slice(), &after_same_len[..]) {
        return process::exit(2)!;
    }

    let smaller: [1]i32 = [20];
    list.replace_range(page, 2, 2, &smaller[..]).exit().?;
    let after_smaller: [4]i32 = [0, 10, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_smaller[..]) {
        return process::exit(3)!;
    }

    let larger: [4]i32 = [30, 31, 32, 33];
    list.replace_range(page, 1, 1, &larger[..]).exit().?;
    let after_larger: [7]i32 = [0, 30, 31, 32, 33, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_larger[..]) {
        return process::exit(4)!;
    }

    list.replace_range(page, 2, 3, list.as_slice()).exit().?;
    let after_alias_replace: [11]i32 = [0, 30, 0, 30, 31, 32, 33, 20, 7, 20, 7];
    if not mem::equal[i32](list.as_slice(), &after_alias_replace[..]) {
        return process::exit(5)!;
    }

    list.truncate(6);
    let after_truncate: [6]i32 = [0, 30, 0, 30, 31, 32];
    if not mem::equal[i32](list.as_slice(), &after_truncate[..]) {
        return process::exit(6)!;
    }
    list.truncate(99);
    if list.len() != 6 {
        return process::exit(7)!;
    }

    let mut owned = list.to_owned_slice(page).exit().?;
    if not mem::equal[i32](owned, list.as_slice()) {
        return process::exit(8)!;
    }
    owned[0] = 1234;
    if list.as_slice()[0] == 1234 {
        return process::exit(9)!;
    }
    page.free_slice[i32](owned).exit().?;

    expect_invalid(list.remove_range(7, 1)).?;
    expect_invalid(list.remove_range(5, 2)).?;
    let invalid_values: [1]i32 = [99];
    expect_invalid(list.replace_range(page, 7, 0, &invalid_values[..])).?;
    expect_invalid(list.replace_range(page, 5, 2, &invalid_values[..])).?;
    if not mem::equal[i32](list.as_slice(), &after_truncate[..]) {
        return process::exit(10)!;
    }

    let mut zero = std::ArrayList[void]::init();
    zero.resize(page, 4).exit().?;
    let mut zero_owned = zero.to_owned_slice(page).exit().?;
    if zero_owned.len() != 4 {
        return process::exit(11)!;
    }
    zero.deinit(page).exit().?;
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
