// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_string_supports_scalar_text_workflows() {
    let root = temp_dir("emit_exe_std_string_supports_scalar_text_workflows");
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let text: &[char] = &"alpha λ beta λ";
    if not text.equals(&"alpha λ beta λ")
        or text.equals(&"alpha λ beta")
        or not text.startsWith(&"alpha λ")
        or text.startsWith(&"lpha")
        or not text.endsWith(&"beta λ")
        or text.endsWith(&"beta")
    {
        return process::ExitCode(1)!;
    }

    if not foundAt(text.find(&"λ"), 6)
        or not foundAt(text.find(&"λ beta"), 6)
        or not foundAt(text.find(&""), 0)
        or not text.contains(&"beta")
        or not text.contains(&"")
        or text.contains(&"gamma")
    {
        return process::ExitCode(2)!;
    }
    if text.find(&"gamma") is ?unexpected {
        _ = unexpected;
        return process::ExitCode(3)!;
    }

    let overlapping: &[char] = &"ababa";
    if not foundAt(overlapping.find(&"aba"), 0)
        or not overlapping.endsWith(&"aba")
        or overlapping.startsWith(&"bab")
    {
        return process::ExitCode(4)!;
    }

    let empty: &[char] = &"";
    if not empty.equals(&"")
        or not empty.startsWith(&"")
        or not empty.endsWith(&"")
        or not foundAt(empty.find(&""), 0)
    {
        return process::ExitCode(5)!;
    }
    if empty.find(&"a") is ?unexpected {
        _ = unexpected;
        return process::ExitCode(5)!;
    }

    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut owned = std::String::fromSlice(page, text).?;
    defer owned.deinit(page).?;
    if not owned.equals(text)
        or not owned.startsWith(&"alpha")
        or not owned.endsWith(&"λ")
        or not foundAt(owned.find(&"beta"), 8)
        or not owned.contains(&"λ beta")
    {
        return process::ExitCode(6)!;
    }

    owned.append(page, &"!").?;
    if not owned.endsWith(&"λ!") or owned.equals(text) {
        return process::ExitCode(7)!;
    }

    owned.reserve(page, 3).?;
    let reservedCapacity = owned.capacity();
    owned.appendAssumeCapacity(&"++");
    owned.pushAssumeCapacity('?');
    if not owned.endsWith(&"!++?") or owned.capacity() != reservedCapacity {
        return process::ExitCode(7)!;
    }

    let mut fieldIndex = 0;
    for field in owned.split(&" ") {
        let matches = if fieldIndex == 0 {
            field.equals(&"alpha")
        } else if fieldIndex == 1 {
            field.equals(&"λ")
        } else if fieldIndex == 2 {
            field.equals(&"beta")
        } else if fieldIndex == 3 {
            field.equals(&"λ!++?")
        } else {
            false
        };
        if not matches {
            return process::ExitCode(7)!;
        }
        fieldIndex += 1;
    }
    if fieldIndex != 4 {
        return process::ExitCode(7)!;
    }

    let mut replacedBorrowed = text.replaceAll(page, &"λ", &"nia").?;
    defer replacedBorrowed.deinit(page).?;
    if not replacedBorrowed.equals(&"alpha nia beta nia") or not text.equals(&"alpha λ beta λ") {
        return process::ExitCode(19)!;
    }

    let aliasReplacement = &owned.text()[0..5];
    let mut replacedOwned = owned.replaceAll(page, &"λ", aliasReplacement).?;
    defer replacedOwned.deinit(page).?;
    if not replacedOwned.equals(&"alpha alpha beta alpha!++?")
        or not owned.equals(&"alpha λ beta λ!++?")
    {
        return process::ExitCode(20)!;
    }

    let repeated: &[char] = &"--a----b--";
    let mut removed = repeated.replaceAll(page, &"--", &"").?;
    defer removed.deinit(page).?;
    if not removed.equals(&"ab") {
        return process::ExitCode(21)!;
    }

    let mut unchanged = text.replaceAll(page, &"", &"ignored").?;
    defer unchanged.deinit(page).?;
    unchanged.textMut()[0] = 'A';
    if not unchanged.equals(&"Alpha λ beta λ") or not text.equals(&"alpha λ beta λ") {
        return process::ExitCode(22)!;
    }

    let mut tinyStorage: [u8; 4] = [0; 4];
    let mut tiny = mem::FixedBufferAllocator::init(&mut tinyStorage);
    let growthSource: &[char] = &"aaa";
    match growthSource.replaceAll(&mut tiny, &"a", &"zz") {
        !result => {
            let mut unexpected = result;
            unexpected.deinit(&mut tiny).?;
            return process::ExitCode(23)!;
        },
        mem::Error::OutOfMemory! => {},
        err! => {
            _ = err;
            return process::ExitCode(24)!;
        },
    }
    if not growthSource.equals(&"aaa") {
        return process::ExitCode(25)!;
    }

    let mut literalReplaced = (&"aba").replaceAll(page, &"a", &"x").?;
    defer literalReplaced.deinit(page).?;
    if not literalReplaced.equals(&"xbx") {
        return process::ExitCode(26)!;
    }

    let parts: [&[char]; 4] = [&"left", &"λ", &"", &"right"];
    let mut joined = (&parts).join(page, &"|").?;
    defer joined.deinit(page).?;
    if not joined.equals(&"left|λ||right") or joined.capacity() != joined.len() {
        return process::ExitCode(27)!;
    }

    let mut concatenated = (&parts).join(page, &"").?;
    defer concatenated.deinit(page).?;
    if not concatenated.equals(&"leftλright") {
        return process::ExitCode(28)!;
    }

    let emptyParts: [&[char]; 0] = [];
    let mut emptyJoined = (&emptyParts).join(page, &"ignored").?;
    defer emptyJoined.deinit(page).?;
    if not emptyJoined.isEmpty() or emptyJoined.capacity() != 0 {
        return process::ExitCode(29)!;
    }

    let singlePart: [&[char]; 1] = [text];
    let mut singleJoined = (&singlePart).join(page, &"ignored").?;
    defer singleJoined.deinit(page).?;
    singleJoined.textMut()[0] = 'A';
    if not singleJoined.equals(&"Alpha λ beta λ") or not text.equals(&"alpha λ beta λ") {
        return process::ExitCode(30)!;
    }

    let borrowedParts: [&[char]; 2] = [text, owned.text()];
    let mut joinedBorrowed = (&borrowedParts).join(page, &" / ").?;
    defer joinedBorrowed.deinit(page).?;
    joinedBorrowed.textMut()[0] = 'A';
    if not joinedBorrowed.equals(&"Alpha λ beta λ / alpha λ beta λ!++?")
        or not text.equals(&"alpha λ beta λ")
        or not owned.equals(&"alpha λ beta λ!++?")
    {
        return process::ExitCode(31)!;
    }

    let mut joinTinyStorage: [u8; 4] = [0; 4];
    let mut joinTiny = mem::FixedBufferAllocator::init(&mut joinTinyStorage);
    let largeParts: [&[char]; 2] = [&"aaa", &"bbb"];
    match (&largeParts).join(&mut joinTiny, &"--") {
        !result => {
            let mut unexpected = result;
            unexpected.deinit(&mut joinTiny).?;
            return process::ExitCode(32)!;
        },
        mem::Error::OutOfMemory! => {},
        err! => {
            _ = err;
            return process::ExitCode(33)!;
        },
    }
    if not largeParts[0].equals(&"aaa") or not largeParts[1].equals(&"bbb") {
        return process::ExitCode(34)!;
    }

    let mut lambdaCount = 0;
    for &ch in text.iter() {
        if ch == 'λ' {
            lambdaCount += 1;
        }
    }
    if lambdaCount != 2 {
        return process::ExitCode(8)!;
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
        return process::ExitCode(9)!;
    }

    let mut ownedText = std::String::fromSlice(page, text).?;
    defer ownedText.deinit(page).?;
    let mut ownedHasher = hash::Wyhash::init(17u64);
    ownedText.hash(&mut ownedHasher);
    if borrowedHash != ownedHasher.finish() {
        return process::ExitCode(10)!;
    }

    let mut equalText = std::String::fromSlice(page, text).?;
    defer equalText.deinit(page).?;
    let mut differentText = std::String::fromSlice(page, &"alpha λ beta").?;
    defer differentText.deinit(page).?;
    if ownedText != equalText or ownedText == differentText {
        return process::ExitCode(11)!;
    }

    let stored = std::String::fromSlice(page, text).?;
    let mut map = std::HashMap[std::String, i32]::initSeedCapacity(
        page,
        23u64,
        1,
    ).?;
    defer map.deinit(page).?;
    let initialInsert = map.insertAssumeCapacity(stored, 41);
    if initialInsert is ?unexpected {
        _ = unexpected;
        return process::ExitCode(12)!;
    }
    let equalIncoming = std::String::fromSlice(page, text).?;
    let replacementInsert = map.insertAssumeCapacity(equalIncoming, 41);
    if replacementInsert is ?replacement {
        if replacement.replacedValue().* != 41 {
            return process::ExitCode(12)!;
        }
        let mut rejectedKey = replacement.intoRejectedKey();
        if not rejectedKey.equals(text) {
            return process::ExitCode(12)!;
        }
        rejectedKey.deinit(page).?;
    } else {
        return process::ExitCode(12)!;
    }
    let absentIncoming = std::String::fromSlice(page, text).?;
    let absentInsert = map.insertIfAbsentAssumeCapacity(absentIncoming, 99);
    if absentInsert is ?rejected {
        if rejected.value().* != 99 {
            return process::ExitCode(12)!;
        }
        let mut rejectedKey = rejected.intoKey();
        rejectedKey.deinit(page).?;
    } else {
        return process::ExitCode(12)!;
    }
    let entryIncoming = std::String::fromSlice(page, text).?;
    let mut entryResult = map.getOrInsertAssumeCapacity(entryIncoming, 100);
    if entryResult.intoRejected() is ?rejected {
        if rejected.value().* != 100 {
            return process::ExitCode(12)!;
        }
        let mut entryRejectedKey = rejected.intoKey();
        entryRejectedKey.deinit(page).?;
    } else {
        return process::ExitCode(12)!;
    }
    if entryResult.value().* != 41 {
        return process::ExitCode(12)!;
    }
    if not map.containsKeyBy(text) {
        return process::ExitCode(13)!;
    }
    if map.getBy(text) is ?value {
        if value.* != 41 {
            return process::ExitCode(14)!;
        }
    } else {
        return process::ExitCode(15)!;
    }
    if map.getMutBy(text) is ?value {
        value.* = 42;
    } else {
        return process::ExitCode(15)!;
    }
    if map.getEntryBy(text) is ?entry {
        if not entry.key().equals(text) or entry.value().* != 42 {
            return process::ExitCode(15)!;
        }
    } else {
        return process::ExitCode(15)!;
    }
    if map.getEntryMutBy(text) is ?value {
        let mut entry = value;
        entry.valueMut().* = 43;
    } else {
        return process::ExitCode(15)!;
    }
    if map.getKeyBy(text) is ?key {
        if not key.equals(text) {
            return process::ExitCode(15)!;
        }
    } else {
        return process::ExitCode(15)!;
    }
    if map.getBy[&[char]](&"missing") is ?unexpected {
        _ = unexpected;
        return process::ExitCode(15)!;
    }

    let mut removedKey: std::String;
    if map.removeEntryBy(text) is ?entry {
        if entry.value().* != 43 {
            return process::ExitCode(16)!;
        }
        removedKey = entry.intoKey();
    } else {
        return process::ExitCode(16)!;
    }
    removedKey.deinit(page).?;
    if not map.isEmpty() {
        return process::ExitCode(17)!;
    }

    map.reserve(page, 2).?;
    let drainOne = std::String::fromSlice(page, &"drain one").?;
    let drainTwo = std::String::fromSlice(page, &"drain two").?;
    if map.insertAssumeCapacity(drainOne, 50) is ?unexpected {
        _ = unexpected;
        return process::ExitCode(18)!;
    }
    if map.insertAssumeCapacity(drainTwo, 70) is ?unexpected {
        _ = unexpected;
        return process::ExitCode(18)!;
    }
    let mut drainedCount = 0;
    let mut drainedTotal = 0;
    for entry in map.drain() {
        drainedCount += 1;
        drainedTotal += entry.value().*;
        let mut key = entry.intoKey();
        key.deinit(page).?;
    }
    if drainedCount != 2 or drainedTotal != 120 or not map.isEmpty() {
        return process::ExitCode(18)!;
    }

    let mut ownedPage = mem::PageAllocator::init();
    let mut ownedAllocator = mem::GeneralPurposeAllocator::init(&mut ownedPage);
    let mut ownedMap = std::HashMap[std::String, std::String]::initSeed(29u64);
    ownedMap.reserve(&mut ownedAllocator, 2).?;

    let storedKey = std::String::fromSlice(&mut ownedAllocator, &"owned").?;
    let storedValue = std::String::fromSlice(&mut ownedAllocator, &"first").?;
    if ownedMap.insertAssumeCapacity(storedKey, storedValue) is ?unexpected {
        _ = unexpected;
        return process::ExitCode(19)!;
    }

    let replacementKey = std::String::fromSlice(&mut ownedAllocator, &"owned").?;
    let replacementValue = std::String::fromSlice(&mut ownedAllocator, &"second").?;
    if ownedMap.insertAssumeCapacity(replacementKey, replacementValue) is ?replacement {
        let mut parts = replacement;
        if not parts.rejectedKey().equals(&"owned")
            or not parts.replacedValue().equals(&"first")
        {
            return process::ExitCode(20)!;
        }
        parts.rejectedKeyMut().deinit(&mut ownedAllocator).?;
        parts.replacedValueMut().deinit(&mut ownedAllocator).?;
    } else {
        return process::ExitCode(21)!;
    }

    let absentKey = std::String::fromSlice(&mut ownedAllocator, &"owned").?;
    let absentValue = std::String::fromSlice(&mut ownedAllocator, &"third").?;
    if ownedMap.insertIfAbsentAssumeCapacity(absentKey, absentValue) is ?rejected {
        let mut parts = rejected;
        parts.keyMut().deinit(&mut ownedAllocator).?;
        parts.valueMut().deinit(&mut ownedAllocator).?;
    } else {
        return process::ExitCode(22)!;
    }

    let entryKey = std::String::fromSlice(&mut ownedAllocator, &"owned").?;
    let entryValue = std::String::fromSlice(&mut ownedAllocator, &"fourth").?;
    let ownedEntryResult = ownedMap.getOrInsertAssumeCapacity(entryKey, entryValue);
    if ownedEntryResult.intoRejected() is ?rejected {
        let mut parts = rejected;
        parts.keyMut().deinit(&mut ownedAllocator).?;
        parts.valueMut().deinit(&mut ownedAllocator).?;
    } else {
        return process::ExitCode(23)!;
    }

    let otherKey = std::String::fromSlice(&mut ownedAllocator, &"other").?;
    let otherValue = std::String::fromSlice(&mut ownedAllocator, &"fifth").?;
    if ownedMap.insertAssumeCapacity(otherKey, otherValue) is ?unexpected {
        _ = unexpected;
        return process::ExitCode(24)!;
    }

    let mut ownedDrained = 0usize;
    for mut entry in ownedMap.drain() {
        entry.keyMut().deinit(&mut ownedAllocator).?;
        entry.valueMut().deinit(&mut ownedAllocator).?;
        ownedDrained += 1;
    }
    if ownedDrained != 2 or not ownedMap.isEmpty() {
        return process::ExitCode(25)!;
    }
    ownedMap.deinit(&mut ownedAllocator).?;
    if ownedAllocator.deinit().? != mem::DeinitStatus::Ok {
        return process::ExitCode(26)!;
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
fn emit_exe_std_text_composes_with_files_and_processes() {
    let root = temp_dir("emit_exe_std_text_composes_with_files_and_processes");
    let data_path = root.join("nia-工作流-42.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::fmt;
using std::fs;
using std::io;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    let mut pageAllocator = mem::PageAllocator::init();
    let page: &mut mem::Allocator = &mut pageAllocator;

    let input: [u8; 6] = [0xe4u8, 0xbdu8, 0xa0u8, 0xe5u8, 0xa5u8, 0xbdu8];
    let utf8 = match std::unicode::Utf8View::fromBytes(&input) {
        !value => value,
        error! => {
            _ = error;
            return process::ExitCode(1)!;
        },
    };
    if utf8.byteLen() != 6 or utf8.scalarCount() != 2 or utf8.isEmpty() {
        return process::ExitCode(18)!;
    }
    let mut scalarIndex: usize = 0;
    for scalar in utf8 {
        if scalarIndex == 0 and scalar.codepoint() != 0x4f60 {
            return process::ExitCode(19)!;
        }
        if scalarIndex == 1 and scalar.codepoint() != 0x597d {
            return process::ExitCode(20)!;
        }
        scalarIndex += 1;
    }
    if scalarIndex != utf8.scalarCount() {
        return process::ExitCode(21)!;
    }
    let mut utf8Iter = utf8.iter();
    if utf8Iter.len() != 2 or utf8Iter.isEmpty() {
        return process::ExitCode(27)!;
    }
    if utf8Iter.next() is ?firstScalar {
        if firstScalar.codepoint() != 0x4f60 or utf8Iter.len() != 1 {
            return process::ExitCode(28)!;
        }
    } else {
        return process::ExitCode(29)!;
    }
    _ = utf8Iter.next();
    if not utf8Iter.isEmpty() {
        return process::ExitCode(30)!;
    }
    if utf8Iter.next() is ?unexpectedScalar {
        _ = unexpectedScalar;
        return process::ExitCode(31)!;
    }
    let emptyUtf8 = match std::unicode::Utf8View::fromBytes(&b"") {
        !value => value,
        error! => {
            _ = error;
            return process::ExitCode(22)!;
        },
    };
    if not emptyUtf8.isEmpty() or emptyUtf8.scalarCount() != 0 {
        return process::ExitCode(23)!;
    }
    let mut content = std::String::fromUtf8View(page, utf8).?;
    defer content.deinit(page).?;
    let suffixUtf8 = match std::unicode::Utf8View::fromBytes(&b" / Nia") {
        !value => value,
        error! => {
            _ = error;
            return process::ExitCode(26)!;
        },
    };
    content.appendUtf8View(page, suffixUtf8).?;
    let answer = 42;
    let contentArgs: [&fmt::Format; 1] = [&answer];
    match content.appendFormat(page, &" #{}", &contentArgs) {
        !ok => { _ = ok; },
        error! => {
            _ = error;
            return process::ExitCode(2)!;
        },
    }

    let truncated: [u8; 2] = [0xe2u8, 0x82u8];
    match std::unicode::Utf8View::fromBytes(&truncated) {
        !value => {
            _ = value;
            return process::ExitCode(24)!;
        },
        std::unicode::Utf8DecodeError::Truncated! => {},
        error! => {
            _ = error;
            return process::ExitCode(25)!;
        },
    }
    match std::String::fromUtf8(page, &truncated) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit(page).?;
            return process::ExitCode(3)!;
        },
        std::TextError::InvalidUtf8(std::unicode::Utf8DecodeError::Truncated)! => {},
        error! => {
            _ = error;
            return process::ExitCode(4)!;
        },
    }

    let invalid: [u8; 1] = [0xffu8];
    match std::String::fromUtf8(page, &invalid) {
        !value => {
            let mut unexpected = value;
            unexpected.deinit(page).?;
            return process::ExitCode(5)!;
        },
        std::TextError::InvalidUtf8(std::unicode::Utf8DecodeError::InvalidLeadingByte)! => {},
        error! => {
            _ = error;
            return process::ExitCode(6)!;
        },
    }

    let mut tinyStorage: [u8; 1] = [0];
    let mut tiny = mem::FixedBufferAllocator::init(&mut tinyStorage[..]);
    match std::String::fromUtf8(&mut tiny, &b"allocation") {
        !value => {
            let mut unexpected = value;
            unexpected.deinit(&mut tiny).?;
            return process::ExitCode(7)!;
        },
        std::TextError::Allocation(mem::Error::OutOfMemory)! => {},
        error! => {
            _ = error;
            return process::ExitCode(8)!;
        },
    }

    let mut pathText = std::String::fromSlice(page, &"nia-工作流-").?;
    let mut pathTextTransferred = false;
    defer if not pathTextTransferred {
        pathText.deinit(page).?;
    };
    let pathArgs: [&fmt::Format; 1] = [&answer];
    match pathText.appendFormat(page, &"{}.txt", &pathArgs) {
        !ok => { _ = ok; },
        error! => {
            _ = error;
            return process::ExitCode(9)!;
        },
    }
    let mut path = fs::Path::fromString(pathText);
    pathTextTransferred = true;
    defer path.deinit(page).?;

    let mut file = fs::File::create(path.view(), fs::CreateOptions::init()).?;
    let mut fileOpen = true;
    defer if fileOpen {
        file.close().?;
    };
    let mut fileBuffer: [u8; 5] = [0; 5];
    let mut writer = file.writer(&mut fileBuffer[..]).?;
    writer.writeUtf8(content.text()).?;
    writer.flush().?;
    file.close().?;
    fileOpen = false;

    let missingCommand = process::Command::init(
        fs::PathView::init(&"/definitely/missing/nia-text-workflow"),
        init.env(),
    );
    let mut missingSpawn = missingCommand.spawn();
    match missingSpawn.finish() {
        !child => {
            _ = child;
            return process::ExitCode(12)!;
        },
        process::Error::Spawn(process::SpawnError::Exec(process::SystemError::NotFound))! => {},
        error! => {
            _ = error;
            return process::ExitCode(13)!;
        },
    }

    let arguments: [&[char]; 1] = [path.text()];
    let command = process::Command::init(fs::PathView::init(&"/bin/cat"), init.env())
        .withArguments(&arguments)
        .withStdout(process::StdIo::Pipe);
    let mut spawn = command.spawn();
    let mut child = spawn.finish().?;
    let mut childLive = true;
    defer if childLive {
        let term = child.kill().?;
        _ = term;
    };
    let mut stdout = match child.takeStdout() {
        ?value => value,
        null => return process::ExitCode(14)!,
    };
    let mut stdoutOpen = true;
    defer if stdoutOpen {
        stdout.close().?;
    };
    let mut readBuffer: [u8; 5] = [0; 5];
    let mut encoded: [u8; 16] = [0; 16];
    {
        let mut reader = stdout.buffered(&mut readBuffer[..]);
        reader.readExact(&mut encoded[..]).?;
    }
    stdout.close().?;
    stdoutOpen = false;
    let term = child.wait().?;
    childLive = false;
    if not term.succeeded() {
        return process::ExitCode(15)!;
    }

    let mut roundTrip = match std::String::fromUtf8(page, &encoded) {
        !value => value,
        error! => {
            _ = error;
            return process::ExitCode(16)!;
        },
    };
    defer roundTrip.deinit(page).?;
    if not roundTrip.equals(content.text()) or not roundTrip.equals(&"你好 / Nia #42") {
        return process::ExitCode(17)!;
    }
    !()
}
"#,
    )
    .expect("write text workflow source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("compile end-to-end std text workflow");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run end-to-end std text workflow");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read text workflow output"),
        "你好 / Nia #42".as_bytes()
    );
}

#[test]
fn check_std_path_does_not_adopt_raw_owned_text() {
    let root = temp_dir("check_std_path_does_not_adopt_raw_owned_text");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std;

fn main() () {
    let mut text: [char; 3] = ['n', 'i', 'a'];
    _ = std::Path::fromOwnedSlice(&mut text[..]);
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
