// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_tree_map_and_tree_set_are_ordered() {
    let root = temp_dir("emit_exe_std_tree_map_and_tree_set_are_ordered");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::collections;
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut map: collections::TreeMap[i32, i32] = .init();
    let mut reserved: collections::TreeMap[i32, i32] = collections::TreeMap[i32, i32]::initCapacity(
        &mut allocator,
        4,
    ).?;
    _ = reserved.insert(&mut allocator, 1, 10).?;
    if reserved.get(&1) is ?value {
        if value.* != 10 { return process::ExitCode(15)!; }
    } else {
        return process::ExitCode(15)!;
    }
    reserved.deinit(&mut allocator).?;
    if not map.isEmpty() or map.len() != 0 {
        return process::ExitCode(1)!;
    }
    _ = map.insert(&mut allocator, 8, 80).?;
    _ = map.insert(&mut allocator, 2, 20).?;
    _ = map.insert(&mut allocator, 5, 50).?;
    if map.len() != 3 or not map.containsKey(&5) or map.containsKey(&7) {
        return process::ExitCode(2)!;
    }
    if map.get(&2) is ?value {
        if value.* != 20 { return process::ExitCode(3)!; }
    } else {
        return process::ExitCode(3)!;
    }
    if map.insert(&mut allocator, 5, 55).? is ?old {
        if old != 50 { return process::ExitCode(4)!; }
    } else {
        return process::ExitCode(4)!;
    }
    if map.lowerBound(&3) != 1 or map.upperBound(&5) != 2 {
        return process::ExitCode(5)!;
    }
    let mut index = 0;
    for entry in map.iter() {
        if index == 0 and (entry.key().* != 2 or entry.value().* != 20) {
            return process::ExitCode(6)!;
        }
        if index == 1 and (entry.key().* != 5 or entry.value().* != 55) {
            return process::ExitCode(7)!;
        }
        if index == 2 and (entry.key().* != 8 or entry.value().* != 80) {
            return process::ExitCode(8)!;
        }
        index += 1;
    }
    if index != 3 { return process::ExitCode(9)!; }
    if map.remove(&5) is ?removed {
        if removed.key().* != 5 or removed.value().* != 55 {
            return process::ExitCode(10)!;
        }
    } else {
        return process::ExitCode(10)!;
    }

    let mut set: collections::TreeSet[i32] = .init();
    if not set.insert(&mut allocator, 9).? or not set.insert(&mut allocator, 3).? {
        return process::ExitCode(11)!;
    }
    if set.insert(&mut allocator, 9).? or not set.contains(&3) or set.contains(&4) {
        return process::ExitCode(12)!;
    }
    let mut setIndex = 0;
    for value in set.iter() {
        if (setIndex == 0 and value.* != 3) or (setIndex == 1 and value.* != 9) {
            return process::ExitCode(13)!;
        }
        setIndex += 1;
    }
    if setIndex != 2 or not set.remove(&3) or set.remove(&3) {
        return process::ExitCode(14)!;
    }
    map.deinit(&mut allocator).?;
    set.deinit(&mut allocator).?;
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
        .output_timeout_for_build("run nia emit --exe std tree map");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn generic_nominal_arraylist_argument_has_sized_layout() {
    let root = temp_dir("generic_nominal_arraylist_argument_has_sized_layout");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Entry[K, V]
where K: Sized, V: Sized
{
    key: K,
    value: V,
}

struct Wrapper[K, V]
where K: Sized, V: Sized, Entry[K, V]: Sized
{
    entries: std::collections::ArrayList[Entry[K, V]],
}

pub fun main() () {
    let _ = Wrapper[i32, i32] { entries: .init() };
}
"#,
    )
    .expect("write regression source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_build("check generic nominal Sized layout");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
