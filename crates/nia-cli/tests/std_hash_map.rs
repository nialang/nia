// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_hash_wyhash_matches_test_vectors() {
    let root = temp_dir("emit_exe_std_hash_wyhash_matches_test_vectors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::hash;
using std::process;

fn expect(seed: u64, input: &[u8], expected: u64, code: i32) process::ExitCode!() {
    let actual = hash::wyhash(seed, input);
    if actual != expected {
        return process::exit(code)!;
    }
    !()
}

fn expectStream(seed: u64, input: &[u8], splitAt: usize, code: i32) process::ExitCode!() {
    let expected = hash::wyhash(seed, input);

    let mut one = hash::Wyhash::init(seed);
    one.update(input);
    if one.finish() != expected or one.finish() != expected {
        return process::exit(code)!;
    }

    let mut split = hash::Wyhash::init(seed);
    split.update(&input[0..splitAt]);
    split.update(&input[splitAt..]);
    if split.finish() != expected {
        return process::exit(code + 1)!;
    }

    let mut bytewise = hash::Wyhash::init(seed);
    let mut i = 0usize;
    while i < input.len() {
        bytewise.update(&input[i..(i + 1)]);
        i += 1;
    }
    if bytewise.finish() != expected {
        return process::exit(code + 2)!;
    }

    let mut chunks = hash::Wyhash::init(seed);
    i = 0;
    while i < input.len() {
        let mut end = i + 7;
        if end > input.len() {
            end = input.len();
        }
        chunks.update(&input[i..end]);
        i = end;
    }
    if chunks.finish() != expected {
        return process::exit(code + 3)!;
    }
    !()
}

fn hashBytes(seed: u64, bytes: &[u8]) u64 {
    let mut hasher = hash::Wyhash::init(seed);
    bytes.len().hash(&mut hasher);
    hasher.write(bytes);
    hasher.finish()
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    expect(0u64, &b"", 0x0409638ee2bde459u64, 1).?;
    expect(1u64, &b"a", 0xa8412d091b5fe0a9u64, 2).?;
    expect(2u64, &b"abc", 0x32dd92e4b2915153u64, 3).?;
    expect(3u64, &b"message digest", 0x8619124089a3a16bu64, 4).?;
    expect(4u64, &b"abcdefghijklmnopqrstuvwxyz", 0x7a43afb61d7f5f40u64, 5).?;
    expect(
        5u64,
        &b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
        0xff42329b90e50d58u64,
        6,
    ).?;
    expect(
        6u64,
        &b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
        0xc39cab13b115aad3u64,
        7,
    ).?;

    let long = b"12345678901234567890123456789012345678901234567890123456789012345678901234567890";
    let expected = hash::wyhash(6u64, &long);

    let mut one = hash::Wyhash::init(6u64);
    one.update(&long);
    if one.finish() != expected or one.finish() != expected {
        return process::exit(8)!;
    }

    let mut split = hash::Wyhash::init(6u64);
    split.update(&long[0..1]);
    split.update(&long[1..17]);
    split.update(&long[17..48]);
    split.update(&long[48..49]);
    split.update(&long[49..]);
    if split.finish() != expected {
        return process::exit(9)!;
    }

    let mut bytewise = hash::Wyhash::init(6u64);
    let mut i = 0usize;
    while i < long.len() {
        bytewise.update(&long[i..(i + 1)]);
        i += 1;
    }
    if bytewise.finish() != expected {
        return process::exit(10)!;
    }

    let boundary = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-*/";
    expectStream(9u64, &boundary[0..0], 0, 20).?;
    expectStream(9u64, &boundary[0..1], 0, 24).?;
    expectStream(9u64, &boundary[0..15], 7, 28).?;
    expectStream(9u64, &boundary[0..16], 8, 32).?;
    expectStream(9u64, &boundary[0..17], 9, 36).?;
    expectStream(9u64, &boundary[0..47], 23, 40).?;
    expectStream(9u64, &boundary[0..48], 24, 44).?;
    expectStream(9u64, &boundary[0..49], 25, 48).?;
    expectStream(9u64, &boundary[0..63], 31, 52).?;
    expectStream(9u64, &boundary[0..64], 32, 56).?;
    expectStream(9u64, &boundary[0..65], 33, 60).?;

    let pair: [u8; 2] = [1u8, 2u8];
    let sliceHash = hashBytes(12u64, &pair[..]);
    let mut rawHasher = hash::Wyhash::init(12u64);
    rawHasher.write(&pair);
    if sliceHash == rawHasher.finish() {
        return process::exit(70)!;
    }

    let mut manualSliceHasher = hash::Wyhash::init(12u64);
    (2usize).hash(&mut manualSliceHasher);
    manualSliceHasher.write(&pair);
    if sliceHash != manualSliceHasher.finish() {
        return process::exit(71)!;
    }

    let mut intHasher = hash::Wyhash::init(13u64);
    (0x01020304u32).hash(&mut intHasher);
    let little_endian: [u8; 4] = [4u8, 3u8, 2u8, 1u8];
    if intHasher.finish() != hash::wyhash(13u64, &little_endian) {
        return process::exit(72)!;
    }

    let mut boolTrue = hash::Wyhash::init(14u64);
    true.hash(&mut boolTrue);
    let mut oneByte = hash::Wyhash::init(14u64);
    (1u8).hash(&mut oneByte);
    if boolTrue.finish() != oneByte.finish() {
        return process::exit(73)!;
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
fn emit_exe_std_hash_map_initialization_is_ergonomic_and_typed() {
    let root = temp_dir("emit_exe_std_hash_map_initialization_is_ergonomic_and_typed");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::collections;
using std::process;

struct Context {}

extend Context {
    fn init() Context {
        {}
    }
}

extend Context : collections::HashMapContext[i32] {
    fn hash(&self, seed: u64, key: &i32) u64 {
        _ = self;
        seed ^ (key.* as u64)
    }

    fn eql(&self, left: &i32, right: &i32) bool {
        _ = self;
        left.* == right.*
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let direct = std::HashMap[i32, i32]::init();
    if not direct.isEmpty() {
        return process::exit(1)!;
    }
    let fallible = match std::HashMap[i32, i32]::tryInit() {
        !map => map,
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    };
    if not fallible.isEmpty() {
        return process::exit(3)!;
    }
    let contextual = collections::HashMapWithContext[i32, i32, Context]::initContext(Context::init());
    if not contextual.isEmpty() {
        return process::exit(4)!;
    }
    let typed = match collections::HashMapWithContext[i32, i32, Context]::tryInitContext(Context::init()) {
        !map => map,
        std::HashMapInitError::System! => return process::exit(5)!,
        error! => {
            _ = error;
            return process::exit(6)!;
        },
    };
    if not typed.isEmpty() {
        return process::exit(7)!;
    }
    !()
}
"#,
    )
    .expect("write hash map initialization source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit hash map initialization executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run hash map initialization executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_hash_map_supports_basic_operations() {
    let root = temp_dir("emit_exe_std_hash_map_supports_basic_operations");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::collections;
using std::HashMap;
using std::hash;
using std::mem;
using std::process;

struct Unit {}

struct ConstantHashContext {}

struct UnitContext {}

struct FailAllocator {
    backing: mem::FixedBufferAllocator,
    allocCount: usize,
    freeCount: usize,
    failAllocAt: usize,
    failFreeAt: usize,
}

struct Key {
    value: i32,
}

struct ModuloContext {
    modulus: i32,
    salt: u64,
}

struct TailHashContext {
    offset: usize,
}

extend ConstantHashContext {
    fn init() ConstantHashContext {
        {}
    }
}

extend UnitContext {
    fn init() UnitContext {
        {}
    }
}

extend FailAllocator {
    fn init(buffer: &mut [u8]) FailAllocator {
        Self {
            backing: mem::FixedBufferAllocator::init(buffer),
            allocCount: 0,
            freeCount: 0,
            failAllocAt: 0,
            failFreeAt: 0,
        }
    }

    fn failNextAlloc(&mut self) () {
        self.failAllocAt = self.allocCount + 1;
    }

    fn failNextFree(&mut self) () {
        self.failFreeAt = self.freeCount + 1;
    }

    fn clearFailures(&mut self) () {
        self.failAllocAt = 0;
        self.failFreeAt = 0;
    }
}

extend Key {
    fn init(value: i32) Key {
        Self { value }
    }
}

extend ModuloContext {
    fn init(modulus: i32, salt: u64) ModuloContext {
        Self { modulus, salt }
    }
}

extend TailHashContext {
    fn init(offset: usize) TailHashContext {
        Self { offset }
    }
}

extend ConstantHashContext : std::collections::HashMapContext[i32] {
    fn hash(&self, seed: u64, key: &i32) u64 {
        _ = self;
        _ = seed;
        _ = key;
        1u64
    }

    fn eql(&self, left: &i32, right: &i32) bool {
        _ = self;
        left.* == right.*
    }
}

extend UnitContext : std::collections::HashMapContext[Unit] {
    fn hash(&self, seed: u64, key: &Unit) u64 {
        _ = self;
        _ = seed;
        _ = key;
        0u64
    }

    fn eql(&self, left: &Unit, right: &Unit) bool {
        _ = self;
        _ = left;
        _ = right;
        true
    }
}

extend FailAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if not layout.isEmpty() {
            self.allocCount += 1;
            if self.failAllocAt == self.allocCount {
                return mem::Error::OutOfMemory!;
            }
        }
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        if not block.isEmpty() {
            self.freeCount += 1;
            if self.failFreeAt == self.freeCount {
                return mem::Error::Invalid!;
            }
        }
        self.backing.free(block)
    }

    fn resize(&mut self, block: mem::Block, newLayout: mem::Layout) bool {
        self.backing.resize(block, newLayout)
    }

    fn remap(&mut self, block: mem::Block, newLayout: mem::Layout) ?mem::Block {
        self.backing.remap(block, newLayout)
    }
}

extend ModuloContext : std::collections::HashMapContext[Key] {
    fn hash(&self, seed: u64, key: &Key) u64 {
        ((key.value % self.modulus) as u64) + seed + self.salt
    }

    fn eql(&self, left: &Key, right: &Key) bool {
        (left.value % self.modulus) == (right.value % self.modulus)
    }
}

extend TailHashContext : std::collections::HashMapContext[i32] {
    fn hash(&self, seed: u64, key: &i32) u64 {
        _ = seed;
        (key.* as u64) + (self.offset as u64)
    }

    fn eql(&self, left: &i32, right: &i32) bool {
        left.* == right.*
    }
}

fn run(init: process::Init) mem::Error!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut gpa = mem::GeneralPurposeAllocator::init(&mut page);
    defer gpa.deinit().ok().?;

    let mut map = std::HashMap[i32, i32]::initSeed(1234u64);
    defer map.deinit(&mut gpa).?;

    map.reserve(&mut gpa, 64).?;
    if map.capacity() < 64 {
        return mem::Error::Invalid!;
    }

    if map.len() != 0 or not map.isEmpty() {
        return mem::Error::Invalid!;
    }

    let mut i = 0;
    while i < 64 {
        let result = map.insert(&mut gpa, i, i * 10).?;
        if result is ?unexpected {
            _ = unexpected;
            return mem::Error::Invalid!;
        }
        i += 1;
    }

    if map.len() != 64 or map.isEmpty() {
        return mem::Error::Invalid!;
    }
    if not map.containsKey(&42) {
        return mem::Error::Invalid!;
    }
    match map.get(&42) {
        ?value => { if value.* != 420 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let replacedAnswer = map.insert(&mut gpa, 42, 7).?;
    if replacedAnswer is ?replacement {
        if replacement.intoReplacedValue() != 420 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    match map.getMut(&42) {
        mut ?value => { value.* = value.* + 1; },
        null => { return mem::Error::Invalid!; },
    }
    match map.get(&42) {
        ?value => { if value.* != 8 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match map.getEntry(&42) {
        ?entry => { if entry.key().* != 42 or entry.value().* != 8 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    match map.getEntryMut(&42) {
        mut ?entry => { if entry.key().* != 42 or entry.value().* != 8 {
                    return mem::Error::Invalid!;
                }
                entry.valueMut().* = 9; },
        null => { return mem::Error::Invalid!; },
    }
    match map.get(&42) {
        ?value => { if value.* != 9 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    for value in map.valuesMut() {
        value.* = value.* + 1;
    }
    match map.get(&42) {
        ?value => { if value.* != 10 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    for mut entry in map.iterMut() {
        if entry.key().* == 42 {
            entry.valueMut().* = entry.value().* + 2;
        }
    }
    match map.get(&42) {
        ?value => { if value.* != 12 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match map.remove(&10) {
        ?value => { if value != 101 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    if map.containsKey(&10) or map.len() != 63 {
        return mem::Error::Invalid!;
    }

    let mut keySum = 0;
    for &key in map.keys() {
        keySum += key;
        if key == 10 {
            return mem::Error::Invalid!;
        }
    }
    if keySum != 2006 {
        return mem::Error::Invalid!;
    }

    let mut valueSum = 0;
    for &value in map.values() {
        valueSum += value;
    }
    if valueSum != 19714 {
        return mem::Error::Invalid!;
    }

    let mut entryCount = 0usize;
    for entry in map.iter() {
        entryCount += 1;
        if entry.key().* == 42 and entry.value().* != 12 {
            return mem::Error::Invalid!;
        }
    }
    if entryCount != map.len() {
        return mem::Error::Invalid!;
    }
    let mut directEntryCount = 0usize;
    for entry in map {
        directEntryCount += 1;
        if entry.key().* == 42 and entry.value().* != 12 {
            return mem::Error::Invalid!;
        }
    }
    if directEntryCount != map.len() {
        return mem::Error::Invalid!;
    }

    match map.removeEntry(&42) {
        ?entry => { if entry.key().* != 42 or entry.value().* != 12 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    if map.containsKey(&42) or map.len() != 62 {
        return mem::Error::Invalid!;
    }

    let firstSeventyFour = map.insert(&mut gpa, 74, 740).?;
    if firstSeventyFour is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    match map.get(&74) {
        ?value => { if value.* != 740 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let replacedSeventyFour = map.insert(&mut gpa, 74, 741).?;
    if replacedSeventyFour is ?replacement {
        if replacement.intoReplacedValue() != 740 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    match map.removeEntry(&74) {
        ?entry => { if entry.key().* != 74 or entry.value().* != 741 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }

    map.clear();
    if map.len() != 0 or map.containsKey(&42) {
        return mem::Error::Invalid!;
    }

    let vacantFive = map.insertIfAbsent(&mut gpa, 5, 50).?;
    if vacantFive is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    let occupiedFive = map.insertIfAbsent(&mut gpa, 5, 500).?;
    if occupiedFive is ?rejected {
        if rejected.key().* != 5 or rejected.value().* != 500 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    match map.get(&5) {
        ?value => { if value.* != 50 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let mut insertedEntry = map.getOrInsert(&mut gpa, 6, 60).?;
    if insertedEntry.intoRejected() is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    if insertedEntry.key().* != 6 {
        return mem::Error::Invalid!;
    }
    insertedEntry.value().* = 61;
    let mut existingEntry = map.getOrInsert(&mut gpa, 6, 600).?;
    if existingEntry.intoRejected() is ?rejected {
        if rejected.key().* != 6 or rejected.value().* != 600 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    if existingEntry.value().* != 61 {
        return mem::Error::Invalid!;
    }
    existingEntry.value().* = 62;
    match map.get(&6) {
        ?value => { if value.* != 62 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let mut rawEntry = map.getOrInsert(&mut gpa, 8, 80).?;
    if rawEntry.intoRejected() is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    rawEntry = map.getOrInsert(&mut gpa, 8, 800).?;
    if rawEntry.intoRejected() is ?rejected {
        if rejected.key().* != 8 or rejected.value().* != 800 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    if rawEntry.value().* != 80 {
        return mem::Error::Invalid!;
    }
    rawEntry.value().* = 81;
    match map.get(&8) {
        ?value => { if value.* != 81 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    map.deinit(&mut gpa).?;
    if map.len() != 0 or map.capacity() != 0 {
        return mem::Error::Invalid!;
    }

    let mut setLike = std::HashMap[i32, Unit]::initSeed(99u64);
    defer setLike.deinit(&mut gpa).?;
    _ = setLike.insert(&mut gpa, 1, {}).?;
    _ = setLike.insert(&mut gpa, 2, {}).?;
    if not setLike.containsKey(&1) or not setLike.containsKey(&2) {
        return mem::Error::Invalid!;
    }
    match setLike.remove(&1) {
        ?value => { _ = value; },
        null => { return mem::Error::Invalid!; },
    }
    if setLike.containsKey(&1) or setLike.len() != 1 {
        return mem::Error::Invalid!;
    }

    let mut unitKeys = std::collections::HashMapWithContext[Unit, i32, UnitContext]::initContextSeed(
        UnitContext::init(),
        0u64,
    );
    defer unitKeys.deinit(&mut gpa).?;
    let firstUnit = unitKeys.insert(&mut gpa, {}, 11).?;
    if firstUnit is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    let replacedUnit = unitKeys.insert(&mut gpa, {}, 22).?;
    if replacedUnit is ?replacement {
        if replacement.intoReplacedValue() != 11 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    if unitKeys.len() != 1 {
        return mem::Error::Invalid!;
    }
    match unitKeys.get(&{}) {
        ?value => { if value.* != 22 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match unitKeys.remove(&{}) {
        ?value => { if value != 22 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    unitKeys.deinit(&mut gpa).?;
    if unitKeys.len() != 0 or unitKeys.capacity() != 0 {
        return mem::Error::Invalid!;
    }

    let mut churn = std::HashMap[i32, i32]::initSeed(555u64);
    defer churn.deinit(&mut gpa).?;
    churn.reserve(&mut gpa, 32).?;
    let churnCapacity = churn.capacity();
    let mut round = 0;
    while round < 4 {
        let mut key = 0;
        while key < 28 {
            _ = churn.insert(&mut gpa, key, key + round).?;
            key += 1;
        }
        key = 0;
        while key < 28 {
            match churn.remove(&key) {
                ?value => { _ = value; },
                null => { return mem::Error::Invalid!; },
            }
            key += 1;
        }
        round += 1;
    }
    if churn.len() != 0 or churn.capacity() != churnCapacity {
        return mem::Error::Invalid!;
    }
    let mut key = 100;
    while key < 128 {
        _ = churn.insert(&mut gpa, key, key * 2).?;
        key += 1;
    }
    if churn.len() != 28 {
        return mem::Error::Invalid!;
    }
    key = 100;
    while key < 128 {
        match churn.get(&key) {
            ?value => { if value.* != key * 2 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    churn.clear();
    if churn.len() != 0 or churn.capacity() != churnCapacity {
        return mem::Error::Invalid!;
    }

    let mut tombstones = std::HashMap[i32, i32]::initSeed(556u64);
    defer tombstones.deinit(&mut gpa).?;
    tombstones.reserve(&mut gpa, 56).?;
    let tombstoneCapacity = tombstones.capacity();
    key = 0;
    while key < 56 {
        _ = tombstones.insert(&mut gpa, key, key).?;
        key += 1;
    }
    key = 0;
    while key < 56 {
        match tombstones.remove(&key) {
            ?value => { if value != key {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.reserve(&mut gpa, 32).?;
    if tombstones.capacity() != tombstoneCapacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        _ = tombstones.insert(&mut gpa, key + 200, key * 4).?;
        key += 1;
    }
    if tombstones.len() != 32 or tombstones.capacity() != tombstoneCapacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        match tombstones.get(&(key + 200)) {
            ?value => { if value.* != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.compact(&mut gpa).?;
    if tombstones.len() != 32 or tombstones.capacity() != tombstoneCapacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        match tombstones.get(&(key + 200)) {
            ?value => { if value.* != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 0;
    while key < 32 {
        match tombstones.remove(&(key + 200)) {
            ?value => { if value != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.compact(&mut gpa).?;
    if tombstones.len() != 0 or tombstones.capacity() != tombstoneCapacity {
        return mem::Error::Invalid!;
    }
    _ = tombstones.insert(&mut gpa, 777, 888).?;
    match tombstones.get(&777) {
        ?value => { if value.* != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrinkToFit(&mut gpa).?;
    if tombstones.len() != 1 or tombstones.capacity() != 7 {
        return mem::Error::Invalid!;
    }
    match tombstones.get(&777) {
        ?value => { if value.* != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrinkToCapacity(&mut gpa, 14).?;
    if tombstones.len() != 1 or tombstones.capacity() != 7 {
        return mem::Error::Invalid!;
    }
    match tombstones.remove(&777) {
        ?value => { if value != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrinkToFit(&mut gpa).?;
    if tombstones.len() != 0 or tombstones.capacity() != 0 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 10 {
        _ = tombstones.insert(&mut gpa, key, key * 11).?;
        key += 1;
    }
    tombstones.reserve(&mut gpa, 64).?;
    if tombstones.capacity() < 64 {
        return mem::Error::Invalid!;
    }
    tombstones.shrinkToCapacity(&mut gpa, 14).?;
    if tombstones.len() != 10 or tombstones.capacity() != 14 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 10 {
        match tombstones.get(&key) {
            ?value => { if value.* != key * 11 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    let mut failStorage: [u8; 8192] = [0; 8192];
    let mut failAllocator = FailAllocator::init(&mut failStorage);
    let mut rollback = std::HashMap[i32, i32]::initSeed(558u64);
    defer rollback.deinit(&mut failAllocator).?;
    rollback.reserve(&mut failAllocator, 14).?;
    key = 0;
    while key < 14 {
        _ = rollback.insert(&mut failAllocator, key, key * 10).?;
        key += 1;
    }
    failAllocator.failNextAlloc();
    match rollback.insert(&mut failAllocator, 99, 990) {
        !old => { _ = old;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollback.len() != 14 or rollback.containsKey(&99) {
        return mem::Error::Invalid!;
    }
    failAllocator.clearFailures();
    key = 0;
    while key < 14 {
        match rollback.get(&key) {
            ?value => { if value.* != key * 10 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    failAllocator.failNextAlloc();
    match rollback.clone(&mut failAllocator) {
        !cloned => { _ = cloned;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    failAllocator.clearFailures();
    failAllocator.failAllocAt = failAllocator.allocCount + 2;
    match rollback.clone(&mut failAllocator) {
        !cloned => { _ = cloned;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollback.len() != 14 or rollback.containsKey(&99) {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        match rollback.get(&key) {
            ?value => { if value.* != key * 10 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    failAllocator.clearFailures();
    _ = rollback.insert(&mut failAllocator, 99, 990).?;
    match rollback.get(&99) {
        ?value => { if value.* != 990 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut rollbackGet = std::HashMap[i32, i32]::initSeed(559u64);
    defer rollbackGet.deinit(&mut failAllocator).?;
    rollbackGet.reserve(&mut failAllocator, 7).?;
    key = 0;
    while key < 7 {
        _ = rollbackGet.insert(&mut failAllocator, key, key).?;
        key += 1;
    }
    failAllocator.failNextAlloc();
    match rollbackGet.getOrInsert(&mut failAllocator, 77, 770) {
        !entry => { _ = entry;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollbackGet.len() != 7 or rollbackGet.containsKey(&77) {
        return mem::Error::Invalid!;
    }
    failAllocator.clearFailures();
    let mut rollbackEntry = rollbackGet.getOrInsert(&mut failAllocator, 77, 770).?;
    if rollbackEntry.intoRejected() is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    match rollbackGet.get(&77) {
        ?value => { if value.* != 770 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut freeFailStorage: [u8; 8192] = [0; 8192];
    let mut freeFailAllocator = FailAllocator::init(&mut freeFailStorage);
    let mut freeFail = std::HashMap[i32, i32]::initSeed(560u64);
    defer freeFail.deinit(&mut freeFailAllocator).?;
    freeFail.reserve(&mut freeFailAllocator, 14).?;
    key = 0;
    while key < 14 {
        _ = freeFail.insert(&mut freeFailAllocator, key, key + 5).?;
        key += 1;
    }
    let oldFreeFailCapacity = freeFail.capacity();
    freeFailAllocator.failNextFree();
    match freeFail.reserve(&mut freeFailAllocator, 64) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if freeFail.capacity() <= oldFreeFailCapacity or freeFail.len() != 14 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        match freeFail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    let freeFailGrownCapacity = freeFail.capacity();
    freeFailAllocator.failNextFree();
    match freeFail.compact(&mut freeFailAllocator) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if freeFail.capacity() != freeFailGrownCapacity or freeFail.len() != 14 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        match freeFail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    freeFailAllocator.clearFailures();
    freeFail.compact(&mut freeFailAllocator).?;
    freeFailAllocator.clearFailures();
    freeFailAllocator.failNextFree();
    match freeFail.shrinkToFit(&mut freeFailAllocator) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if freeFail.capacity() != 14 or freeFail.len() != 14 {
        return mem::Error::Invalid!;
    }
    freeFailAllocator.clearFailures();
    freeFailAllocator.failNextAlloc();
    freeFail.shrinkToCapacity(&mut freeFailAllocator, freeFail.capacity()).?;
    key = 0;
    while key < 14 {
        match freeFail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    freeFailAllocator.clearFailures();
    _ = freeFail.insert(&mut freeFailAllocator, 90, 900).?;
    match freeFail.get(&90) {
        ?value => { if value.* != 900 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut rehashFreeCountStorage: [u8; 8192] = [0; 8192];
    let mut rehashFreeCountAllocator = FailAllocator::init(&mut rehashFreeCountStorage);
    let mut rehashFreeCountMap = std::HashMap[i32, i32]::initSeed(562u64);
    rehashFreeCountMap.reserve(&mut rehashFreeCountAllocator, 14).?;
    key = 0;
    while key < 14 {
        _ = rehashFreeCountMap.insert(&mut rehashFreeCountAllocator, key, key + 11).?;
        key += 1;
    }
    let rehashFreeCountBefore = rehashFreeCountAllocator.freeCount;
    rehashFreeCountAllocator.failNextFree();
    match rehashFreeCountMap.reserve(&mut rehashFreeCountAllocator, 64) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    let rehashFreeCountAfterFailure = rehashFreeCountAllocator.freeCount;
    if rehashFreeCountAfterFailure != rehashFreeCountBefore + 3 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        match rehashFreeCountMap.get(&key) {
            ?value => { if value.* != key + 11 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    rehashFreeCountAllocator.clearFailures();
    rehashFreeCountMap.deinit(&mut rehashFreeCountAllocator).?;
    if rehashFreeCountAllocator.freeCount != rehashFreeCountAfterFailure + 4 {
        return mem::Error::Invalid!;
    }

    let mut deinitStorage: [u8; 8192] = [0; 8192];
    let mut deinitAllocator = FailAllocator::init(&mut deinitStorage);
    let mut retryDeinit = std::HashMap[i32, i32]::initSeed(561u64);
    retryDeinit.reserve(&mut deinitAllocator, 14).?;
    key = 0;
    while key < 14 {
        _ = retryDeinit.insert(&mut deinitAllocator, key, key + 7).?;
        key += 1;
    }
    deinitAllocator.failNextFree();
    match retryDeinit.deinit(&mut deinitAllocator) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    let failedDeinitFreeCount = deinitAllocator.freeCount;
    if failedDeinitFreeCount != 3 {
        return mem::Error::Invalid!;
    }
    deinitAllocator.clearFailures();
    retryDeinit.deinit(&mut deinitAllocator).?;
    if deinitAllocator.freeCount != failedDeinitFreeCount + 1
        or retryDeinit.len() != 0
        or retryDeinit.capacity() != 0
    {
        return mem::Error::Invalid!;
    }

    let mut collisions = std::collections::HashMapWithContext[i32, i32, ConstantHashContext]::initContextSeed(
        ConstantHashContext::init(),
        777u64,
    );
    defer collisions.deinit(&mut gpa).?;
    collisions.reserve(&mut gpa, 16).?;
    key = 0;
    while key < 20 {
        _ = collisions.insert(&mut gpa, key, key + 1000).?;
        key += 1;
    }
    if collisions.len() != 20 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 20 {
        match collisions.get(&key) {
            ?value => { if value.* != key + 1000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 0;
    while key < 10 {
        match collisions.remove(&key) {
            ?value => { if value != key + 1000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 100;
    while key < 110 {
        _ = collisions.insert(&mut gpa, key, key + 2000).?;
        key += 1;
    }
    if collisions.len() != 20 {
        return mem::Error::Invalid!;
    }
    key = 10;
    while key < 20 {
        if not collisions.containsKey(&key) {
            return mem::Error::Invalid!;
        }
        key += 1;
    }
    key = 100;
    while key < 110 {
        match collisions.get(&key) {
            ?value => { if value.* != key + 2000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    let mut modulo = std::collections::HashMapWithContext[Key, i32, ModuloContext]::initContextSeed(
        ModuloContext::init(5, 0x9e3779b97f4a7c15u64),
        19u64,
    );
    defer modulo.deinit(&mut gpa).?;
    let firstModulo = modulo.insert(&mut gpa, Key::init(1), 10).?;
    if firstModulo is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    let replacedModulo = modulo.insert(&mut gpa, Key::init(6), 60).?;
    if replacedModulo is ?replacement {
        if replacement.rejectedKey().*.value != 6
            or replacement.intoReplacedValue() != 10
        {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    if modulo.len() != 1 {
        return mem::Error::Invalid!;
    }
    let equivalent = Key::init(11);
    match modulo.getKey(&equivalent) {
        ?storedKey => { if storedKey.value != 1 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match modulo.getEntry(&equivalent) {
        ?entry => { if entry.key().*.value != 1 or entry.value().* != 60 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    match modulo.get(&equivalent) {
        ?value => { if value.* != 60 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    key = 20;
    while key < 60 {
        _ = modulo.insert(&mut gpa, Key::init(key), key * 3).?;
        key += 1;
    }
    if modulo.len() != 5 {
        return mem::Error::Invalid!;
    }
    let replaced = Key::init(46);
    match modulo.get(&replaced) {
        ?value => { if value.* != 56 * 3 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut tailProbe = std::collections::HashMapWithContext[i32, i32, TailHashContext]::initContextSeed(
        TailHashContext::init(15),
        0u64,
    );
    defer tailProbe.deinit(&mut gpa).?;
    tailProbe.reserve(&mut gpa, 14).?;
    if tailProbe.capacity() != 14 {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 8 {
        _ = tailProbe.insert(&mut gpa, key, key + 100).?;
        key += 1;
    }
    key = 0;
    while key < 8 {
        match tailProbe.get(&key) {
            ?value => { if value.* != key + 100 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    match tailProbe.remove(&0) {
        ?value => { if value != 100 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let tailInsert = tailProbe.insert(&mut gpa, 16, 1600).?;
    if tailInsert is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    match tailProbe.get(&16) {
        ?value => { if value.* != 1600 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tailProbe.clear();
    _ = tailProbe.insert(&mut gpa, 0, 700).?;
    match tailProbe.get(&0) {
        ?value => { if value.* != 700 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut tinyStorage: [u8; 16] = [0; 16];
    let mut tiny = mem::FixedBufferAllocator::init(&mut tinyStorage);
    let mut tinyMap = std::HashMap[i32, i32]::initSeed(11u64);
    defer tinyMap.deinit(&mut tiny).?;
    match tinyMap.reserve(&mut tiny, 64) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }

    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    run(init).exit().?;
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
fn emit_exe_std_hash_map_formats_entries() {
    let root = temp_dir("emit_exe_std_hash_map_formats_entries");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::HashMap;
using std::debug;
using std::fmt;
using std::hash;
using std::mem;
using std::process;

fn run(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut map = std::HashMap[i32, i32]::initSeed(42u64);
    defer map.deinit(&mut page).exit().?;

    _ = map.insert(&mut page, 1, 10).exit().?;
    _ = map.insert(&mut page, 2, 20).exit().?;
    debug::print(&"hash_map={}\n", &[&map]).exit().?;
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    run(init).?;
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

    let output = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("hash_map={"), "{stderr}");
}

#[test]
fn emit_exe_std_hash_map_model_churn_and_clone() {
    let root = temp_dir("emit_exe_std_hash_map_model_churn_and_clone");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::HashMap;
using std::hash;
using std::mem;
using std::process;

fn run(init: process::Init) mem::Error!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut gpa = mem::GeneralPurposeAllocator::init(&mut page);
    defer gpa.deinit().ok().?;

    let mut modelMap = std::HashMap[i32, i32]::initSeed(557u64);
    defer modelMap.deinit(&mut gpa).?;
    let mut expected: [i32; 40] = [0; 40];
    let mut present: [bool; 40] = [false; 40];
    let mut expectedLen = 0usize;
    let mut step = 0;
    while step < 240 {
        let slot = (step * 17 + 3) % 40;
        let op = step % 6;
        if op == 0 {
            let wasPresent = present[slot];
            let result = modelMap.insert(&mut gpa, slot, step + 1000).?;
            if result is ?replacement {
                if not wasPresent or replacement.intoReplacedValue() != expected[slot] {
                    return mem::Error::Invalid!;
                }
            } else if wasPresent {
                return mem::Error::Invalid!;
            }
            if not wasPresent {
                expectedLen += 1;
            }
            present[slot] = true;
            expected[slot] = step + 1000;
        } else if op == 1 {
            let wasPresent = present[slot];
            match modelMap.remove(&slot) {
                ?value => { if not wasPresent or value != expected[slot] {
                            return mem::Error::Invalid!;
                        }
                        present[slot] = false;
                        expectedLen -= 1; },
                null => { if wasPresent {
                        return mem::Error::Invalid!;
                    } },
            }
        } else if op == 2 {
            let mut entry = modelMap.getOrInsert(&mut gpa, slot, step + 2000).?;
            if present[slot] {
                if entry.intoRejected() is ?rejected {
                    if rejected.key().* != slot or rejected.value().* != step + 2000 {
                        return mem::Error::Invalid!;
                    }
                } else {
                    return mem::Error::Invalid!;
                }
                if entry.value().* != expected[slot] {
                    return mem::Error::Invalid!;
                }
            } else {
                if entry.intoRejected() is ?unexpected {
                    _ = unexpected;
                    return mem::Error::Invalid!;
                }
                expectedLen += 1;
                present[slot] = true;
            }
            entry.value().* = step + 2000;
            expected[slot] = step + 2000;
        } else if op == 3 {
            let wasPresent = present[slot];
            let result = modelMap.insertIfAbsent(&mut gpa, slot, step + 3000).?;
            if result is ?rejected {
                if not wasPresent
                    or rejected.key().* != slot
                    or rejected.value().* != step + 3000
                {
                    return mem::Error::Invalid!;
                }
            } else {
                if wasPresent {
                    return mem::Error::Invalid!;
                }
                expectedLen += 1;
                present[slot] = true;
                expected[slot] = step + 3000;
            }
        } else if op == 4 {
            match modelMap.get(&slot) {
                ?value => { if not present[slot] or value.* != expected[slot] {
                        return mem::Error::Invalid!;
                    } },
                null => { if present[slot] {
                        return mem::Error::Invalid!;
                    } },
            }
        } else {
            let wasPresent = present[slot];
            let result = modelMap.insert(&mut gpa, slot, step + 4000).?;
            if result is ?replacement {
                if not wasPresent or replacement.intoReplacedValue() != expected[slot] {
                    return mem::Error::Invalid!;
                }
            } else if wasPresent {
                return mem::Error::Invalid!;
            }
            if not wasPresent {
                expectedLen += 1;
            }
            present[slot] = true;
            expected[slot] = step + 4000;
        }

        if modelMap.len() != expectedLen {
            return mem::Error::Invalid!;
        }
        step += 1;
    }

    let mut modelCount = 0usize;
    let mut modelSum = 0;
    for entry in modelMap.iter() {
        let entryKey = entry.key().*;
        if entryKey < 0 or entryKey >= 40 {
            return mem::Error::Invalid!;
        }
        if not present[entryKey] or entry.value().* != expected[entryKey] {
            return mem::Error::Invalid!;
        }
        modelCount += 1;
        modelSum += entry.value().*;
    }
    if modelCount != expectedLen {
        return mem::Error::Invalid!;
    }

    let mut key = 0;
    let mut expectedSum = 0;
    while key < 40 {
        if present[key] {
            expectedSum += expected[key];
        }
        key += 1;
    }
    if modelSum != expectedSum {
        return mem::Error::Invalid!;
    }

    let mut clonedModel = modelMap.clone(&mut gpa).?;
    defer clonedModel.deinit(&mut gpa).?;
    if clonedModel.len() != modelMap.len() or clonedModel.capacity() != modelMap.capacity() {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 40 {
        match clonedModel.get(&key) {
            ?value => { if not present[key] or value.* != expected[key] {
                    return mem::Error::Invalid!;
                } },
            null => { if present[key] {
                    return mem::Error::Invalid!;
                } },
        }
        key += 1;
    }
    _ = modelMap.insert(&mut gpa, 3, 12345).?;
    match clonedModel.get(&3) {
        ?value => { if value.* == 12345 {
                return mem::Error::Invalid!;
            }; },
        null => { },
    }

    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    run(init).exit().?;
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
fn emit_exe_std_hash_map_reserve_compacts_tombstones() {
    let root = temp_dir("emit_exe_std_hash_map_reserve_compacts_tombstones");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::HashMap;
using std::hash;
using std::mem;
using std::process;

struct FailAllocator {
    backing: mem::FixedBufferAllocator,
    allocCount: usize,
    failAllocAt: usize,
}

extend FailAllocator {
    fn init(buffer: &mut [u8]) FailAllocator {
        Self {
            backing: mem::FixedBufferAllocator::init(buffer),
            allocCount: 0,
            failAllocAt: 0,
        }
    }

    fn failNextAlloc(&mut self) () {
        self.failAllocAt = self.allocCount + 1;
    }
}

extend FailAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if not layout.isEmpty() {
            self.allocCount += 1;
            if self.failAllocAt == self.allocCount {
                return mem::Error::OutOfMemory!;
            }
        }
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!() {
        self.backing.free(block)
    }

    fn resize(&mut self, block: mem::Block, newLayout: mem::Layout) bool {
        self.backing.resize(block, newLayout)
    }
}

fn run(init: process::Init) mem::Error!() {
    _ = init;
    let mut storage: [u8; 32768] = [0; 32768];
    let mut allocator = FailAllocator::init(&mut storage);
    let mut map = std::HashMap[i32, i32]::initSeed(123u64);
    defer map.deinit(&mut allocator).?;

    map.reserve(&mut allocator, 14).?;
    let mut key = 0;
    while key < 14 {
        _ = map.insert(&mut allocator, key, key).?;
        key += 1;
    }

    key = 0;
    while key < 14 {
        match map.remove(&key) {
            ?value => { if value != key {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    map.reserve(&mut allocator, 2).?;
    allocator.failNextAlloc();
    _ = map.insert(&mut allocator, 100, 1000).?;
    _ = map.insert(&mut allocator, 101, 1010).?;

    if map.len() != 2 {
        return mem::Error::Invalid!;
    }
    match map.get(&100) {
        ?value => { if value.* != 1000 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match map.get(&101) {
        ?value => { if value.* != 1010 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    run(init).exit().?;
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
fn emit_exe_std_hash_map_assume_capacity_operations() {
    let root = temp_dir("emit_exe_std_hash_map_assume_capacity_operations");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::HashMap;
using std::hash;
using std::mem;
using std::process;

fn run(init: process::Init) mem::Error!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut map = std::HashMap[i32, i32]::initSeed(321u64);
    defer map.deinit(&mut page).?;

    map.reserve(&mut page, 4).?;
    let firstInsert = map.insertAssumeCapacity(1, 10);
    if firstInsert is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    let replacementInsert = map.insertAssumeCapacity(1, 11);
    if replacementInsert is ?replacement {
        if replacement.intoReplacedValue() != 10 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    let occupiedInsert = map.insertIfAbsentAssumeCapacity(1, 99);
    if occupiedInsert is null {
        return mem::Error::Invalid!;
    }
    let vacantInsert = map.insertIfAbsentAssumeCapacity(2, 20);
    if vacantInsert is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }

    let mut existing = map.getOrInsertAssumeCapacity(1, 111);
    if existing.intoRejected() is ?rejected {
        if rejected.key().* != 1 or rejected.value().* != 111 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }
    if existing.value().* != 11 {
        return mem::Error::Invalid!;
    }
    existing.value().* = 12;

    let mut inserted = map.getOrInsertAssumeCapacity(3, 30);
    if inserted.intoRejected() is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }

    if map.len() != 3 {
        return mem::Error::Invalid!;
    }
    match map.get(&1) {
        ?value => { if value.* != 12 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match map.get(&2) {
        ?value => { if value.* != 20 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    match map.get(&3) {
        ?value => { if value.* != 30 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let retainedCapacity = map.capacity();
    let mut drainedCount = 0;
    let mut drainedTotal = 0;
    for entry in map.drain() {
        drainedCount += 1;
        drainedTotal += entry.value().*;
        break;
    }
    if drainedCount != 1 or map.len() != 2 or map.capacity() != retainedCapacity {
        return mem::Error::Invalid!;
    }
    for entry in map.drain() {
        drainedCount += 1;
        drainedTotal += entry.value().*;
    }
    if drainedCount != 3
        or drainedTotal != 62
        or not map.isEmpty()
        or map.capacity() != retainedCapacity
    {
        return mem::Error::Invalid!;
    }
    if map.insertAssumeCapacity(4, 40) is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    if map.get(&4) is ?value {
        if value.* != 40 {
            return mem::Error::Invalid!;
        }
    } else {
        return mem::Error::Invalid!;
    }

    let mut full = std::HashMap[i32, i32]::initSeed(654u64);
    defer full.deinit(&mut page).?;
    full.reserve(&mut page, 7).?;
    let fullCapacity = full.capacity();
    let mut key = 0;
    while key < 7 {
        if full.insertAssumeCapacity(key, key * 10) is ?unexpected {
            _ = unexpected;
            return mem::Error::Invalid!;
        }
        key += 1;
    }
    match full.remove(&3) {
        ?value => { if value != 30 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    if full.insertAssumeCapacity(7, 70) is ?unexpected {
        _ = unexpected;
        return mem::Error::Invalid!;
    }
    if full.len() != fullCapacity {
        return mem::Error::Invalid!;
    }
    full.reserve(&mut page, 1).?;
    if full.capacity() <= fullCapacity {
        return mem::Error::Invalid!;
    }
    match full.get(&7) {
        ?value => { if value.* != 70 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
    run(init).exit().?;
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
