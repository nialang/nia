// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_string_compares_searches_and_hashes_scalar_text() {
    let root = temp_dir("emit_exe_std_string_compares_searches_and_hashes_scalar_text");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::collections;
using std::hash;
using std::mem;
using std::process;
using std::string;

fn foundAt(result: ?usize, expected: usize) bool {
    if result is ?index {
        index == expected
    } else {
        false
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let text: &[char] = &"alpha λ beta λ";
    if not text.equals(&"alpha λ beta λ")
        or text.equals(&"alpha λ beta")
        or not text.startsWith(&"alpha λ")
        or text.startsWith(&"lpha")
        or not text.endsWith(&"beta λ")
        or text.endsWith(&"beta")
    {
        return process::exit(1)!;
    }

    if not foundAt(text.find(&"λ"), 6)
        or not foundAt(text.find(&"λ beta"), 6)
        or not foundAt(text.find(&""), 0)
        or not text.contains(&"beta")
        or not text.contains(&"")
        or text.contains(&"gamma")
    {
        return process::exit(2)!;
    }
    if text.find(&"gamma") is ?unexpected {
        _ = unexpected;
        return process::exit(3)!;
    }

    let overlapping: &[char] = &"ababa";
    if not foundAt(overlapping.find(&"aba"), 0)
        or not overlapping.endsWith(&"aba")
        or overlapping.startsWith(&"bab")
    {
        return process::exit(4)!;
    }

    let empty: &[char] = &"";
    if not empty.equals(&"")
        or not empty.startsWith(&"")
        or not empty.endsWith(&"")
        or not foundAt(empty.find(&""), 0)
    {
        return process::exit(5)!;
    }
    if empty.find(&"a") is ?unexpected {
        _ = unexpected;
        return process::exit(5)!;
    }

    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut owned = std::String::fromSlice(page, text).exit().?;
    defer owned.deinit(page).exit().?;
    if not owned.equals(text)
        or not owned.startsWith(&"alpha")
        or not owned.endsWith(&"λ")
        or not foundAt(owned.find(&"beta"), 8)
        or not owned.contains(&"λ beta")
    {
        return process::exit(6)!;
    }

    owned.append(page, &"!").exit().?;
    if not owned.endsWith(&"λ!") or owned.equals(text) {
        return process::exit(7)!;
    }

    owned.reserve(page, 3).exit().?;
    let reservedCapacity = owned.capacity();
    owned.appendAssumeCapacity(&"++");
    owned.pushAssumeCapacity('?');
    if not owned.endsWith(&"!++?") or owned.capacity() != reservedCapacity {
        return process::exit(7)!;
    }

    let mut lambdaCount = 0;
    for &ch in text.iter() {
        if ch == 'λ' {
            lambdaCount += 1;
        }
    }
    if lambdaCount != 2 {
        return process::exit(8)!;
    }

    let mut borrowedHasher = hash::Wyhash::init(17u64);
    text.hash(&mut borrowedHasher);
    let borrowedHash = borrowedHasher.finish();

    let mut manualHasher = hash::Wyhash::init(17u64);
    text.len().hash(&mut manualHasher);
    for &ch in text.iter() {
        ch.hash(&mut manualHasher);
    }
    if borrowedHash != manualHasher.finish() {
        return process::exit(9)!;
    }

    let mut ownedText = std::String::fromSlice(page, text).exit().?;
    defer ownedText.deinit(page).exit().?;
    let mut ownedHasher = hash::Wyhash::init(17u64);
    ownedText.hash(&mut ownedHasher);
    if borrowedHash != ownedHasher.finish() {
        return process::exit(10)!;
    }

    let mut equalText = std::String::fromSlice(page, text).exit().?;
    defer equalText.deinit(page).exit().?;
    let mut differentText = std::String::fromSlice(page, &"alpha λ beta").exit().?;
    defer differentText.deinit(page).exit().?;
    if ownedText != equalText or ownedText == differentText {
        return process::exit(11)!;
    }

    let stored = std::String::fromSlice(page, text).exit().?;
    let mut map = std::HashMap[std::String, i32]::initSeedCapacity(
        page,
        23u64,
        1,
    ).exit().?;
    defer map.deinit(page).exit().?;
    let initialInsert = map.insertAssumeCapacity(stored, 41);
    if initialInsert is ?unexpected {
        _ = unexpected;
        return process::exit(12)!;
    }
    let equalIncoming = std::String::fromSlice(page, text).exit().?;
    let replacementInsert = map.insertAssumeCapacity(equalIncoming, 41);
    if replacementInsert is ?replacement {
        if replacement.replacedValue().* != 41 {
            return process::exit(12)!;
        }
        let mut rejectedKey = replacement.intoRejectedKey();
        if not rejectedKey.equals(text) {
            return process::exit(12)!;
        }
        rejectedKey.deinit(page).exit().?;
    } else {
        return process::exit(12)!;
    }
    let absentIncoming = std::String::fromSlice(page, text).exit().?;
    let absentInsert = map.insertIfAbsentAssumeCapacity(absentIncoming, 99);
    if absentInsert is ?rejected {
        if rejected.value().* != 99 {
            return process::exit(12)!;
        }
        let mut rejectedKey = rejected.intoKey();
        rejectedKey.deinit(page).exit().?;
    } else {
        return process::exit(12)!;
    }
    let entryIncoming = std::String::fromSlice(page, text).exit().?;
    let mut entryResult = map.getOrInsertAssumeCapacity(entryIncoming, 100);
    if entryResult.intoRejected() is ?rejected {
        if rejected.value().* != 100 {
            return process::exit(12)!;
        }
        let mut entryRejectedKey = rejected.intoKey();
        entryRejectedKey.deinit(page).exit().?;
    } else {
        return process::exit(12)!;
    }
    if entryResult.value().* != 41 {
        return process::exit(12)!;
    }
    if not map.containsKeyBy(text) {
        return process::exit(13)!;
    }
    if map.getBy(text) is ?value {
        if value.* != 41 {
            return process::exit(14)!;
        }
    } else {
        return process::exit(15)!;
    }
    if map.getMutBy(text) is ?value {
        value.* = 42;
    } else {
        return process::exit(15)!;
    }
    if map.getEntryBy(text) is ?entry {
        if not entry.key().equals(text) or entry.value().* != 42 {
            return process::exit(15)!;
        }
    } else {
        return process::exit(15)!;
    }
    if map.getEntryMutBy(text) is ?value {
        let mut entry = value;
        entry.valueMut().* = 43;
    } else {
        return process::exit(15)!;
    }
    if map.getKeyBy(text) is ?key {
        if not key.equals(text) {
            return process::exit(15)!;
        }
    } else {
        return process::exit(15)!;
    }
    if map.getBy[&[char]](&"missing") is ?unexpected {
        _ = unexpected;
        return process::exit(15)!;
    }

    let mut removedKey: std::String;
    if map.removeEntryBy(text) is ?entry {
        if entry.value().* != 43 {
            return process::exit(16)!;
        }
        removedKey = entry.intoKey();
    } else {
        return process::exit(16)!;
    }
    removedKey.deinit(page).exit().?;
    if not map.isEmpty() {
        return process::exit(17)!;
    }

    map.reserve(page, 2).exit().?;
    let drainOne = std::String::fromSlice(page, &"drain one").exit().?;
    let drainTwo = std::String::fromSlice(page, &"drain two").exit().?;
    if map.insertAssumeCapacity(drainOne, 50) is ?unexpected {
        _ = unexpected;
        return process::exit(18)!;
    }
    if map.insertAssumeCapacity(drainTwo, 70) is ?unexpected {
        _ = unexpected;
        return process::exit(18)!;
    }
    let mut drainedCount = 0;
    let mut drainedTotal = 0;
    for entry in map.drain() {
        drainedCount += 1;
        drainedTotal += entry.value().*;
        let mut key = entry.intoKey();
        key.deinit(page).exit().?;
    }
    if drainedCount != 2 or drainedTotal != 120 or not map.isEmpty() {
        return process::exit(18)!;
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
fn check_std_path_buf_does_not_adopt_raw_owned_text() {
    let root = temp_dir("check_std_path_buf_does_not_adopt_raw_owned_text");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std;

fn main() void {
    let mut text: [3]char = ['n', 'i', 'a'];
    _ = std::PathBuf::fromOwnedSlice(&mut text[..]);
}
"#,
    )
    .expect("write obsolete path ownership source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("check obsolete path ownership API");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fromOwnedSlice"), "{stderr}");
}
