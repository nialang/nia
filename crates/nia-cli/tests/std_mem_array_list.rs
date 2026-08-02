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
    switch std::ArrayList[i32]::initCapacity(page, 3) {
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
    let insertedTail: [2]i32 = [4, 5];
    switch ops.insertSlice(page, 3, &insertedTail[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(29)!; },
    }
    let expectedOps: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.asSlice(), &expectedOps[..]) {
        return process::exit(30)!;
    }
    switch ops.orderedRemove(1) {
        ?value => { if value != 2 {
                    return process::exit(31)!;
                } },
        null => { return process::exit(32)!; },
    }
    let expectedOrdered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.asSlice(), &expectedOrdered[..]) {
        return process::exit(33)!;
    }
    switch ops.swapRemove(0) {
        ?value => { if value != 1 {
                    return process::exit(34)!;
                } },
        null => { return process::exit(35)!; },
    }
    let expectedSwap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.asSlice(), &expectedSwap[..]) {
        return process::exit(36)!;
    }
    switch ops.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(37)!; },
    }

    let mut alias = std::ArrayList[i32]::init();
    switch alias.reserveExact(page, 2) {
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
    switch alias.appendSlice(page, alias.asSlice()) {
        !ok => { _ = ok; },
        error! => { return process::exit(41)!; },
    }
    let expectedAliasAppend: [4]i32 = [1, 2, 1, 2];
    if not mem::equal[i32](alias.asSlice(), &expectedAliasAppend[..]) {
        return process::exit(42)!;
    }
    switch alias.insertSlice(page, 1, alias.asSlice()) {
        !ok => { _ = ok; },
        error! => { return process::exit(43)!; },
    }
    let expectedAliasInsert: [8]i32 = [1, 1, 2, 1, 2, 2, 1, 2];
    if not mem::equal[i32](alias.asSlice(), &expectedAliasInsert[..]) {
        return process::exit(44)!;
    }
    switch alias.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(45)!; },
    }

    let mut iterList = std::ArrayList[i32]::init();
    switch iterList.push(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(75)!; },
    }
    switch iterList.push(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(76)!; },
    }
    switch iterList.push(page, 3) {
        !ok => { _ = ok; },
        error! => { return process::exit(77)!; },
    }
    for value in iterList.iterMut() {
        value.* = value.* * 2;
    }
    for value in iterList.iterMut().rev().take(2) {
        value.* += 1;
    }
    let mut iterSum = 0;
    for &value in iterList {
        iterSum += value;
    }
    if iterSum != 14 {
        return process::exit(80)!;
    }
    let expectedIterMut: [3]i32 = [2, 5, 7];
    if not mem::equal[i32](iterList.asSlice(), &expectedIterMut[..]) {
        return process::exit(78)!;
    }
    switch iterList.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(79)!; },
    }

    let mut list = std::ArrayList[i32]::init();
    if list.len() != 0 or not list.isEmpty() {
        return process::exit(4)!;
    }
    switch list.reserveExact(page, 2) {
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
    let items = list.asSlice();
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
    switch list.getMut(4) {
        mut ?value => { value.* = 44; },
        null => { return process::exit(71)!; },
    }
    switch list.lastMut() {
        mut ?value => { value.* = 55; },
        null => { return process::exit(72)!; },
    }
    let expectedAfterAccessors: [6]i32 = [0, 10, 20, 30, 44, 55];
    if not mem::equal[i32](list.asSlice(), &expectedAfterAccessors[..]) {
        return process::exit(73)!;
    }

    let more: [3]i32 = [60, 70, 80];
    switch list.appendSlice(page, &more[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(12)!; },
    }
    if list.len() != 9 or list.asSlice()[8] != 80 {
        return process::exit(13)!;
    }

    switch list.push(page, 90) {
        !ok => { _ = ok; },
        error! => { return process::exit(14)!; },
    }
    if list.len() != 10 or list.asSlice()[9] != 90 {
        return process::exit(15)!;
    }

    let added: [2]i32 = [100, 110];
    switch list.appendSlice(page, &added[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(16)!; },
    }
    if list.len() != 12 or list.asSlice()[11] != 110 {
        return process::exit(17)!;
    }

    let inserted: [2]i32 = [21, 22];
    switch list.insertSlice(page, 2, &inserted[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(46)!; },
    }
    if list.len() != 14 or list.asSlice()[2] != 21 or list.asSlice()[3] != 22 or list.asSlice()[4] != 20 {
        return process::exit(47)!;
    }

    switch list.reserveExact(page, 1) {
        !ok => { _ = ok; },
        error! => { return process::exit(48)!; },
    }
    list.appendAssumeCapacity(120);
    if list.len() != 15 or list.asSlice()[14] != 120 {
        return process::exit(48)!;
    }

    list.truncate(10);
    let beforeShrinkCapacity = list.capacity();
    switch list.shrinkToFit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(57)!; },
    }
    if list.len() != 10 or list.capacity() > beforeShrinkCapacity or list.capacity() < list.len() {
        return process::exit(58)!;
    }

    let retainedCapacity = list.capacity();
    list.truncate(10);
    if list.len() != 10 or list.capacity() != retainedCapacity {
        return process::exit(18)!;
    }

    switch list.reserveExact(page, 2) {
        !ok => { _ = ok; },
        error! => { return process::exit(74)!; },
    }
    let tail: [2]i32 = [100, 110];
    list.appendSliceAssumeCapacity(&tail[..]);
    if list.len() != 12 or list.asSlice()[10] != 100 or list.asSlice()[11] != 110 {
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
    let mut mutableItems = list.asMutSlice();
    if mutableItems.len() != list.len() {
        return process::exit(23)!;
    }
    mutableItems[2] = 77;
    if list.asSlice()[2] != 77 {
        return process::exit(24)!;
    }
    list.clear();
    if not list.isEmpty() {
        return process::exit(25)!;
    }
    switch list.deinit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(26)!; },
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
fn emit_exe_std_array_list_preserves_elements_while_shrinking_and_reuses() {
    let root = temp_dir("emit_exe_std_array_list_preserves_elements_while_shrinking_and_reuses");
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
    switch list.shrinkToCapacity(page, 0) {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    if list.len() != 2 or list.capacity() != 2 {
        return process::exit(4)!;
    }
    list.clear();
    switch list.shrinkToFit(page) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
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
    if not mem::equal[i32](list.asSlice(), &expected[..]) {
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
    let mut sourceItems = source.asMutSlice();
    sourceItems[0] = 9;
    let expectedSource: [2]i32 = [9, 2];
    let expectedClone: [2]i32 = [1, 2];
    if not mem::equal[i32](source.asSlice(), &expectedSource[..]) {
        return process::exit(4)!;
    }
    if not mem::equal[i32](cloned.asSlice(), &expectedClone[..]) {
        return process::exit(5)!;
    }

    let mut owned: &mut [i32];
    switch source.intoOwnedSlice(page) {
        !value => { owned = value; },
        error! => { return process::exit(6)!; },
    }
    if source.len() != 0 or source.capacity() != 0 {
        return process::exit(7)!;
    }
    if not mem::equal[i32](owned, &expectedSource[..]) {
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
    let mut adopted = std::ArrayList[i32]::fromOwnedSlice(external);
    let expectedAdopted: [3]i32 = [4, 5, 6];
    if adopted.capacity() != 3 or not mem::equal[i32](adopted.asSlice(), &expectedAdopted[..]) {
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
    switch list.push(page, {}) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
    }
    switch list.push(page, {}) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
    }
    switch list.push(page, {}) {
        !ok => { _ = ok; },
        error! => { return process::exit(4)!; },
    }
    if list.len() != 4 or list.capacity() != usize::MAX {
        return process::exit(5)!;
    }
    list.truncate(3);
    switch list.shrinkToFit(page) {
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

fn expectInvalid(result: mem::Error!void) process::ExitCode!void {
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
    list.appendSlice(page, &initial[..]).exit().?;

    list.removeRange(2, 3).exit().?;
    let afterRemove: [5]i32 = [0, 1, 5, 6, 7];
    if not mem::equal[i32](list.asSlice(), &afterRemove[..]) {
        return process::exit(1)!;
    }

    let sameLen: [2]i32 = [10, 11];
    list.replaceRange(page, 1, 2, &sameLen[..]).exit().?;
    let afterSameLen: [5]i32 = [0, 10, 11, 6, 7];
    if not mem::equal[i32](list.asSlice(), &afterSameLen[..]) {
        return process::exit(2)!;
    }

    let smaller: [1]i32 = [20];
    list.replaceRange(page, 2, 2, &smaller[..]).exit().?;
    let afterSmaller: [4]i32 = [0, 10, 20, 7];
    if not mem::equal[i32](list.asSlice(), &afterSmaller[..]) {
        return process::exit(3)!;
    }

    let larger: [4]i32 = [30, 31, 32, 33];
    list.replaceRange(page, 1, 1, &larger[..]).exit().?;
    let afterLarger: [7]i32 = [0, 30, 31, 32, 33, 20, 7];
    if not mem::equal[i32](list.asSlice(), &afterLarger[..]) {
        return process::exit(4)!;
    }

    list.replaceRange(page, 2, 3, list.asSlice()).exit().?;
    let afterAliasReplace: [11]i32 = [0, 30, 0, 30, 31, 32, 33, 20, 7, 20, 7];
    if not mem::equal[i32](list.asSlice(), &afterAliasReplace[..]) {
        return process::exit(5)!;
    }

    list.truncate(6);
    let afterTruncate: [6]i32 = [0, 30, 0, 30, 31, 32];
    if not mem::equal[i32](list.asSlice(), &afterTruncate[..]) {
        return process::exit(6)!;
    }
    list.truncate(99);
    if list.len() != 6 {
        return process::exit(7)!;
    }

    let mut owned = list.toOwnedSlice(page).exit().?;
    if not mem::equal[i32](owned, list.asSlice()) {
        return process::exit(8)!;
    }
    owned[0] = 1234;
    if list.asSlice()[0] == 1234 {
        return process::exit(9)!;
    }
    page.free_slice[i32](owned).exit().?;

    expectInvalid(list.removeRange(7, 1)).?;
    expectInvalid(list.removeRange(5, 2)).?;
    let invalidValues: [1]i32 = [99];
    expectInvalid(list.replaceRange(page, 7, 0, &invalidValues[..])).?;
    expectInvalid(list.replaceRange(page, 5, 2, &invalidValues[..])).?;
    if not mem::equal[i32](list.asSlice(), &afterTruncate[..]) {
        return process::exit(10)!;
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
