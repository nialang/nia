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
    if reserved.capacity() != 4 { return process::ExitCode(23)!; }
    _ = reserved.insert(&mut allocator, 1, 10).?;
    _ = reserved.insert(&mut allocator, 2, 20).?;
    _ = reserved.insert(&mut allocator, 3, 30).?;
    _ = reserved.insert(&mut allocator, 4, 40).?;
    if reserved.len() != 4 or reserved.capacity() != 4 {
        return process::ExitCode(24)!;
    }
    if reserved.get(&1) is ?value {
        if value.* != 10 { return process::ExitCode(15)!; }
    } else {
        return process::ExitCode(15)!;
    }
    reserved.deinit(&mut allocator).?;
    let mut wide: collections::TreeMap[i32, u128] = .init();
    for key in 0..16 {
        _ = wide.insert(&mut allocator, key, (key as u128) * 1_000_003).?;
    }
    for key in 0..16 {
        if wide.get(&key) is ?value {
            if value.* != (key as u128) * 1_000_003 {
                return process::ExitCode(25)!;
            }
        } else {
            return process::ExitCode(25)!;
        }
    }
    wide.deinit(&mut allocator).?;
    let mut unitValues: collections::TreeMap[i32, ()] = .init();
    for key in 0..9 {
        _ = unitValues.insert(&mut allocator, key, ()).?;
    }
    if unitValues.len() != 9 or not unitValues.containsKey(&8) {
        return process::ExitCode(26)!;
    }
    unitValues.deinit(&mut allocator).?;

    let mut reservedStorage: [u8; 4096] = [0; 4096];
    let mut fixedAllocator = mem::FixedBufferAllocator::init(&mut reservedStorage[..]);
    let mut reservedTree = collections::TreeMap[i32, i32]::initCapacity(&mut fixedAllocator, 24).?;
    let usedAfterReserve = fixedAllocator.used();
    for key in 0..24 {
        _ = reservedTree.insert(&mut fixedAllocator, key, key).?;
    }
    if fixedAllocator.used() != usedAfterReserve {
        return process::ExitCode(27)!;
    }
    reservedTree.deinit(&mut fixedAllocator).?;
    if fixedAllocator.used() != 0 { return process::ExitCode(28)!; }

    let mut partialStorage: [u8; 1024] = [0; 1024];
    let mut partialAllocator = mem::FixedBufferAllocator::init(&mut partialStorage[..]);
    let mut partial: collections::TreeMap[i32, i32] = .init();
    _ = partial.insert(&mut partialAllocator, 1, 10).?;
    _ = partial.insert(&mut partialAllocator, 2, 20).?;
    _ = partial.insert(&mut partialAllocator, 3, 30).?;
    if partial.reserve(&mut partialAllocator, 100) is error! {
        if partial.capacity() != 3 or partial.len() != 3 {
            return process::ExitCode(29)!;
        }
        if partial.get(&1) is ?first {
            if first.* != 10 { return process::ExitCode(32)!; }
        } else {
            return process::ExitCode(32)!;
        }
        if partial.get(&2) is ?second {
            if second.* != 20 { return process::ExitCode(32)!; }
        } else {
            return process::ExitCode(32)!;
        }
        if partial.get(&3) is ?third {
            if third.* != 30 { return process::ExitCode(32)!; }
        } else {
            return process::ExitCode(32)!;
        }
        partial.deinit(&mut partialAllocator).?;
        if partialAllocator.used() != 0 { return process::ExitCode(30)!; }
    } else {
        return process::ExitCode(31)!;
    }
    let mut insertStorage: [u8; 1024] = [0; 1024];
    let mut insertAllocator = mem::FixedBufferAllocator::init(&mut insertStorage[..]);
    let mut atomicInsert: collections::TreeMap[i32, i32] = .init();
    for key in 1..8 {
        _ = atomicInsert.insert(&mut insertAllocator, key, key * 10).?;
    }
    let blocker = insertAllocator.allocBytes(insertAllocator.remaining(), 1).?;
    match atomicInsert.insert(&mut insertAllocator, 8, 80) {
        !old => {
            _ = old;
            return process::ExitCode(33)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return process::ExitCode(34)!;
            }
        },
    }
    if atomicInsert.len() != 7 or atomicInsert.capacity() != 7 or atomicInsert.containsKey(&8) {
        return process::ExitCode(35)!;
    }
    insertAllocator.free(blocker).?;
    atomicInsert.deinit(&mut insertAllocator).?;
    if insertAllocator.used() != 0 { return process::ExitCode(36)!; }
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
fn emit_exe_std_tree_map_splits_merges_and_reuses_nodes() {
    let root = temp_dir("emit_exe_std_tree_map_splits_merges_and_reuses_nodes");
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

    for value in 0..257 {
        let key = (value * 73) % 257;
        _ = map.insert(&mut allocator, key, key * 2).?;
    }
    if map.len() != 257 { return process::ExitCode(20)!; }
    if map.capacity() < 257 { return process::ExitCode(21)!; }
    if map.remove(&300) is ?_ { return process::ExitCode(22)!; }
    for key in 0..257 {
        if map.lowerBound(&key) != key as usize or map.upperBound(&key) != (key + 1) as usize {
            return process::ExitCode(2)!;
        }
        if map.get(&key) is ?value {
            if value.* != key * 2 { return process::ExitCode(3)!; }
        } else {
            return process::ExitCode(3)!;
        }
    }
    if map.lowerBound(&300) != 257 or map.upperBound(&300) != 257 {
        return process::ExitCode(4)!;
    }

    let mut index: i32 = 0;
    for entry in map.iter() {
        if entry.key().* != index or entry.value().* != index * 2 {
            return process::ExitCode(5)!;
        }
        index += 1;
    }
    if index != 257 { return process::ExitCode(6)!; }

    for key in 0..257 {
        if map.getMut(&key) is mut ?value {
            value.* += 1;
        } else {
            return process::ExitCode(7)!;
        }
    }
    let mut mutableIndex: i32 = 0;
    for mut entry in map.iterMut() {
        if entry.key().* != mutableIndex { return process::ExitCode(16)!; }
        entry.valueMut().* += 1;
        mutableIndex += 1;
    }
    if mutableIndex != 257 { return process::ExitCode(17)!; }
    for value in 0..257 {
        let key = (value * 149) % 257;
        if map.remove(&key) is ?removed {
            if removed.key().* != key or removed.value().* != key * 2 + 2 {
                return process::ExitCode(8)!;
            }
        } else {
            return process::ExitCode(8)!;
        }
        if map.containsKey(&key) { return process::ExitCode(9)!; }
        if map.len() != (256 - value) as usize {
            return process::ExitCode(13)!;
        }
        let mut expectedRank: usize = 0;
        for remaining in 0..257 {
            if remaining < key and map.containsKey(&remaining) {
                expectedRank += 1;
            }
        }
        if map.lowerBound(&key) != expectedRank {
            return process::ExitCode(14)!;
        }
        if map.upperBound(&key) != expectedRank {
            return process::ExitCode(18)!;
        }
    }
    if not map.isEmpty() or map.lowerBound(&10) != 0 {
        return process::ExitCode(10)!;
    }
    if map.remove(&0) is ?_ { return process::ExitCode(10)!; }

    map.clear();
    for key in 0..64 {
        _ = map.insert(&mut allocator, 63 - key, key).?;
    }
    index = 0;
    for entry in map.iter() {
        if entry.key().* != index { return process::ExitCode(11)!; }
        index += 1;
    }
    if index != 64 { return process::ExitCode(12)!; }

    map.clear();
    for key in 0..8 {
        _ = map.insert(&mut allocator, key, key).?;
    }
    _ = map.remove(&7);
    if map.remove(&-1) is ?_ {
        return process::ExitCode(15)!;
    }
    if map.len() != 7 or map.lowerBound(&4) != 4 {
        return process::ExitCode(15)!;
    }

    map.deinit(&mut allocator).?;
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
        .output_timeout_for_build("run nia B-tree split merge and reuse");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run emitted B-tree stress executable");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
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
