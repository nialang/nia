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

fn expect(seed: u64, input: &[u8], expected: u64, code: i32) process::ExitCode!void {
    let actual = hash::wyhash(seed, input);
    if actual != expected {
        return (code as process::ExitCode)!;
    }
    !{}
}

fn expect_stream(seed: u64, input: &[u8], split_at: usize, code: i32) process::ExitCode!void {
    let expected = hash::wyhash(seed, input);

    let mut one = hash::Wyhash::init(seed);
    one.update(input);
    if one.finish() != expected or one.finish() != expected {
        return (code as process::ExitCode)!;
    }

    let mut split = hash::Wyhash::init(seed);
    split.update(&input[0usize..split_at]);
    split.update(&input[split_at..]);
    if split.finish() != expected {
        return ((code + 1) as process::ExitCode)!;
    }

    let mut bytewise = hash::Wyhash::init(seed);
    let mut i = 0usize;
    while i < input.len() {
        bytewise.update(&input[i..(i + 1usize)]);
        i += 1usize;
    }
    if bytewise.finish() != expected {
        return ((code + 2) as process::ExitCode)!;
    }

    let mut chunks = hash::Wyhash::init(seed);
    i = 0usize;
    while i < input.len() {
        let mut end = i + 7usize;
        if end > input.len() {
            end = input.len();
        }
        chunks.update(&input[i..end]);
        i = end;
    }
    if chunks.finish() != expected {
        return ((code + 3) as process::ExitCode)!;
    }
    !{}
}

fn hash_bytes(seed: u64, bytes: &[u8]) u64 {
    let mut hasher = hash::Wyhash::init(seed);
    bytes.len().hash(&mut hasher);
    hasher.write(bytes);
    hasher.finish()
}

pub fn main(init: process::Init) process::ExitCode!void {
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
        return (8 as process::ExitCode)!;
    }

    let mut split = hash::Wyhash::init(6u64);
    split.update(&long[0usize..1usize]);
    split.update(&long[1usize..17usize]);
    split.update(&long[17usize..48usize]);
    split.update(&long[48usize..49usize]);
    split.update(&long[49usize..]);
    if split.finish() != expected {
        return (9 as process::ExitCode)!;
    }

    let mut bytewise = hash::Wyhash::init(6u64);
    let mut i = 0usize;
    while i < long.len() {
        bytewise.update(&long[i..(i + 1usize)]);
        i += 1usize;
    }
    if bytewise.finish() != expected {
        return (10 as process::ExitCode)!;
    }

    let boundary = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-*/";
    expect_stream(9u64, &boundary[0usize..0usize], 0usize, 20).?;
    expect_stream(9u64, &boundary[0usize..1usize], 0usize, 24).?;
    expect_stream(9u64, &boundary[0usize..15usize], 7usize, 28).?;
    expect_stream(9u64, &boundary[0usize..16usize], 8usize, 32).?;
    expect_stream(9u64, &boundary[0usize..17usize], 9usize, 36).?;
    expect_stream(9u64, &boundary[0usize..47usize], 23usize, 40).?;
    expect_stream(9u64, &boundary[0usize..48usize], 24usize, 44).?;
    expect_stream(9u64, &boundary[0usize..49usize], 25usize, 48).?;
    expect_stream(9u64, &boundary[0usize..63usize], 31usize, 52).?;
    expect_stream(9u64, &boundary[0usize..64usize], 32usize, 56).?;
    expect_stream(9u64, &boundary[0usize..65usize], 33usize, 60).?;

    let pair: [2]u8 = [1u8, 2u8];
    let slice_hash = hash_bytes(12u64, &pair[..]);
    let mut raw_hasher = hash::Wyhash::init(12u64);
    raw_hasher.write(&pair);
    if slice_hash == raw_hasher.finish() {
        return (70 as process::ExitCode)!;
    }

    let mut manual_slice_hasher = hash::Wyhash::init(12u64);
    (2usize).hash(&mut manual_slice_hasher);
    manual_slice_hasher.write(&pair);
    if slice_hash != manual_slice_hasher.finish() {
        return (71 as process::ExitCode)!;
    }

    let mut int_hasher = hash::Wyhash::init(13u64);
    (0x01020304u32).hash(&mut int_hasher);
    let little_endian: [4]u8 = [4u8, 3u8, 2u8, 1u8];
    if int_hasher.finish() != hash::wyhash(13u64, &little_endian) {
        return (72 as process::ExitCode)!;
    }

    let mut bool_true = hash::Wyhash::init(14u64);
    true.hash(&mut bool_true);
    let mut one_byte = hash::Wyhash::init(14u64);
    (1u8).hash(&mut one_byte);
    if bool_true.finish() != one_byte.finish() {
        return (73 as process::ExitCode)!;
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
fn emit_exe_std_os_random_fills_requested_bytes() {
    let root = temp_dir("emit_exe_std_os_random_fills_requested_bytes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::os;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut empty: [0]u8 = [];
    switch os::random(&mut empty[..]) {
        !ok => { _ = ok; },
        error! => { return (2 as process::ExitCode)!; },
    }

    let mut bytes: [32]u8 = [
        0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    ];
    switch os::random(&mut bytes[..]) {
        !ok => { _ = ok; },
        error! => { return (3 as process::ExitCode)!; },
    }

    let mut any_nonzero = false;
    let mut i = 0usize;
    while i < 32usize {
        if bytes[i] != 0u8 {
            any_nonzero = true;
        }
        i += 1usize;
    }
    if not any_nonzero {
        return (1 as process::ExitCode)!;
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
    alloc_count: usize,
    free_count: usize,
    fail_alloc_at: usize,
    fail_free_at: usize,
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
        {
            backing: mem::FixedBufferAllocator::init(buffer),
            alloc_count: 0usize,
            free_count: 0usize,
            fail_alloc_at: 0usize,
            fail_free_at: 0usize,
        }
    }

    fn fail_next_alloc(&mut self) void {
        self.fail_alloc_at = self.alloc_count + 1usize;
    }

    fn fail_next_free(&mut self) void {
        self.fail_free_at = self.free_count + 1usize;
    }

    fn clear_failures(&mut self) void {
        self.fail_alloc_at = 0usize;
        self.fail_free_at = 0usize;
    }
}

extend Key {
    fn init(value: i32) Key {
        { value: value }
    }
}

extend ModuloContext {
    fn init(modulus: i32, salt: u64) ModuloContext {
        { modulus: modulus, salt: salt }
    }
}

extend TailHashContext {
    fn init(offset: usize) TailHashContext {
        { offset: offset }
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
        if not layout.is_empty() {
            self.alloc_count += 1usize;
            if self.fail_alloc_at == self.alloc_count {
                return mem::Error::OutOfMemory!;
            }
        }
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        if not block.is_empty() {
            self.free_count += 1usize;
            if self.fail_free_at == self.free_count {
                return mem::Error::Invalid!;
            }
        }
        self.backing.free(block)
    }

    fn resize(&mut self, block: mem::Block, new_layout: mem::Layout) bool {
        self.backing.resize(block, new_layout)
    }

    fn remap(&mut self, block: mem::Block, new_layout: mem::Layout) ?mem::Block {
        self.backing.remap(block, new_layout)
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

fn run(init: process::Init) mem::Error!void {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut gpa = mem::GeneralPurposeAllocator::init(&mut page);
    defer gpa.deinit().ok().?;

    let mut map = std::HashMap[i32, i32]::init_seed(1234u64);
    defer map.deinit(&mut gpa).?;

    map.reserve(&mut gpa, 64usize).?;
    if map.capacity() < 64usize {
        return mem::Error::Invalid!;
    }

    if map.len() != 0usize or not map.is_empty() {
        return mem::Error::Invalid!;
    }

    let mut i = 0;
    while i < 64 {
        switch map.put(&mut gpa, i, i * 10) {
            !old => { switch old {
         ?value => { return mem::Error::Invalid!; },
         null => { },
     }; },
            err! => { return err!; },
        }
        i += 1;
    }

    if map.len() != 64usize or map.is_empty() {
        return mem::Error::Invalid!;
    }
    if not map.contains_key(&42) {
        return mem::Error::Invalid!;
    }
    switch map.get(&42) {
        ?value => { if value.* != 420 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    switch map.put(&mut gpa, 42, 7) {
        !old => { switch old {
         ?value => { if value != 420 {
                        return mem::Error::Invalid!;
                    } },
         null => { return mem::Error::Invalid!; },
     }; },
        err! => { return err!; },
    }
    switch map.get_mut(&42) {
        mut ?value => { value.* = value.* + 1; },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get(&42) {
        ?value => { if value.* != 8 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get_entry(&42) {
        ?entry => { if entry.key().* != 42 or entry.value().* != 8 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get_entry_mut(&42) {
        mut ?entry => { if entry.key().* != 42 or entry.value().* != 8 {
                    return mem::Error::Invalid!;
                }
                entry.value_mut().* = 9; },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get(&42) {
        ?value => { if value.* != 9 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    for value in map.values_mut() {
        value.* = value.* + 1;
    }
    switch map.get(&42) {
        ?value => { if value.* != 10 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    for mut entry in map.iter_mut() {
        if entry.key().* == 42 {
            entry.value_mut().* = entry.value().* + 2;
        }
    }
    switch map.get(&42) {
        ?value => { if value.* != 12 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.remove(&10) {
        ?value => { if value != 101 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    if map.contains_key(&10) or map.len() != 63usize {
        return mem::Error::Invalid!;
    }

    let mut key_sum = 0;
    for &key in map.keys() {
        key_sum += key;
        if key == 10 {
            return mem::Error::Invalid!;
        }
    }
    if key_sum != 2006 {
        return mem::Error::Invalid!;
    }

    let mut value_sum = 0;
    for &value in map.values() {
        value_sum += value;
    }
    if value_sum != 19714 {
        return mem::Error::Invalid!;
    }

    let mut entry_count = 0usize;
    for entry in map.iter() {
        entry_count += 1usize;
        if entry.key().* == 42 and entry.value().* != 12 {
            return mem::Error::Invalid!;
        }
    }
    if entry_count != map.len() {
        return mem::Error::Invalid!;
    }
    let mut direct_entry_count = 0usize;
    for entry in map {
        direct_entry_count += 1usize;
        if entry.key().* == 42 and entry.value().* != 12 {
            return mem::Error::Invalid!;
        }
    }
    if direct_entry_count != map.len() {
        return mem::Error::Invalid!;
    }

    switch map.remove_entry(&42) {
        ?entry => { if entry.key().* != 42 or entry.value().* != 12 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    if map.contains_key(&42) or map.len() != 62usize {
        return mem::Error::Invalid!;
    }

    switch map.put(&mut gpa, 74, 740) {
        !old => { switch old {
         ?value => { return mem::Error::Invalid!; },
         null => { },
     }; },
        err! => { return err!; },
    }
    switch map.get(&74) {
        ?value => { if value.* != 740 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.fetch_put(&mut gpa, 74, 741) {
        !old => { switch old {
         ?value => { if value != 740 {
                        return mem::Error::Invalid!;
                    } },
         null => { return mem::Error::Invalid!; },
     }; },
        err! => { return err!; },
    }
    switch map.fetch_remove(&74) {
        ?entry => { if entry.key().* != 74 or entry.value().* != 741 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }

    map.clear();
    if map.len() != 0usize or map.contains_key(&42) {
        return mem::Error::Invalid!;
    }

    switch map.put_if_absent(&mut gpa, 5, 50) {
        !inserted => { if not inserted {
                return mem::Error::Invalid!;
            } },
        err! => { return err!; },
    }
    switch map.put_if_absent(&mut gpa, 5, 500) {
        !inserted => { if inserted {
                return mem::Error::Invalid!;
            } },
        err! => { return err!; },
    }
    switch map.get(&5) {
        ?value => { if value.* != 50 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let mut inserted_entry = map.get_or_put_value(&mut gpa, 6, 60).?;
    if inserted_entry.found_existing() or inserted_entry.key().* != 6 {
        return mem::Error::Invalid!;
    }
    inserted_entry.value().* = 61;
    let mut existing_entry = map.get_or_put_value(&mut gpa, 6, 600).?;
    if not existing_entry.found_existing() or existing_entry.value().* != 61 {
        return mem::Error::Invalid!;
    }
    existing_entry.value().* = 62;
    switch map.get(&6) {
        ?value => { if value.* != 62 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    let mut raw_entry = map.get_or_put(&mut gpa, 8).?;
    if raw_entry.found_existing() {
        return mem::Error::Invalid!;
    }
    raw_entry.value().* = 80;
    raw_entry = map.get_or_put(&mut gpa, 8).?;
    if not raw_entry.found_existing() or raw_entry.value().* != 80 {
        return mem::Error::Invalid!;
    }
    raw_entry.value().* = 81;
    switch map.get(&8) {
        ?value => { if value.* != 81 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    map.clear_and_free(&mut gpa).?;
    if map.len() != 0usize or map.capacity() != 0usize {
        return mem::Error::Invalid!;
    }

    let mut set_like = std::HashMap[i32, Unit]::init_seed(99u64);
    defer set_like.deinit(&mut gpa).?;
    _ = set_like.put(&mut gpa, 1, {}).?;
    _ = set_like.put(&mut gpa, 2, {}).?;
    if not set_like.contains_key(&1) or not set_like.contains_key(&2) {
        return mem::Error::Invalid!;
    }
    switch set_like.remove(&1) {
        ?value => { _ = value; },
        null => { return mem::Error::Invalid!; },
    }
    if set_like.contains_key(&1) or set_like.len() != 1usize {
        return mem::Error::Invalid!;
    }

    let mut unit_keys = std::collections::HashMapWithContext[Unit, i32, UnitContext]::init_context_seed(
        UnitContext::init(),
        0u64,
    );
    defer unit_keys.deinit(&mut gpa).?;
    switch unit_keys.put(&mut gpa, {}, 11) {
        !old => { switch old {
         ?value => { return mem::Error::Invalid!; },
         null => { },
     }; },
        err! => { return err!; },
    }
    switch unit_keys.put(&mut gpa, {}, 22) {
        !old => { switch old {
         ?value => { if value != 11 {
                        return mem::Error::Invalid!;
                    } },
         null => { return mem::Error::Invalid!; },
     }; },
        err! => { return err!; },
    }
    if unit_keys.len() != 1usize {
        return mem::Error::Invalid!;
    }
    switch unit_keys.get(&{}) {
        ?value => { if value.* != 22 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch unit_keys.remove(&{}) {
        ?value => { if value != 22 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    unit_keys.clear_and_free(&mut gpa).?;
    if unit_keys.len() != 0usize or unit_keys.capacity() != 0usize {
        return mem::Error::Invalid!;
    }

    let mut churn = std::HashMap[i32, i32]::init_seed(555u64);
    defer churn.deinit(&mut gpa).?;
    churn.reserve(&mut gpa, 32usize).?;
    let churn_capacity = churn.capacity();
    let mut round = 0;
    while round < 4 {
        let mut key = 0;
        while key < 28 {
            _ = churn.put(&mut gpa, key, key + round).?;
            key += 1;
        }
        key = 0;
        while key < 28 {
            switch churn.remove(&key) {
                ?value => { _ = value; },
                null => { return mem::Error::Invalid!; },
            }
            key += 1;
        }
        round += 1;
    }
    if churn.len() != 0usize or churn.capacity() != churn_capacity {
        return mem::Error::Invalid!;
    }
    let mut key = 100;
    while key < 128 {
        _ = churn.put(&mut gpa, key, key * 2).?;
        key += 1;
    }
    if churn.len() != 28usize {
        return mem::Error::Invalid!;
    }
    key = 100;
    while key < 128 {
        switch churn.get(&key) {
            ?value => { if value.* != key * 2 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    churn.clear_retaining_capacity();
    if churn.len() != 0usize or churn.capacity() != churn_capacity {
        return mem::Error::Invalid!;
    }

    let mut tombstones = std::HashMap[i32, i32]::init_seed(556u64);
    defer tombstones.deinit(&mut gpa).?;
    tombstones.reserve(&mut gpa, 56usize).?;
    let tombstone_capacity = tombstones.capacity();
    key = 0;
    while key < 56 {
        _ = tombstones.put(&mut gpa, key, key).?;
        key += 1;
    }
    key = 0;
    while key < 56 {
        switch tombstones.remove(&key) {
            ?value => { if value != key {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.reserve(&mut gpa, 32usize).?;
    if tombstones.capacity() != tombstone_capacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        _ = tombstones.put(&mut gpa, key + 200, key * 4).?;
        key += 1;
    }
    if tombstones.len() != 32usize or tombstones.capacity() != tombstone_capacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        switch tombstones.get(&(key + 200)) {
            ?value => { if value.* != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.compact(&mut gpa).?;
    if tombstones.len() != 32usize or tombstones.capacity() != tombstone_capacity {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 32 {
        switch tombstones.get(&(key + 200)) {
            ?value => { if value.* != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 0;
    while key < 32 {
        switch tombstones.remove(&(key + 200)) {
            ?value => { if value != key * 4 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    tombstones.compact(&mut gpa).?;
    if tombstones.len() != 0usize or tombstones.capacity() != tombstone_capacity {
        return mem::Error::Invalid!;
    }
    _ = tombstones.put(&mut gpa, 777, 888).?;
    switch tombstones.get(&777) {
        ?value => { if value.* != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrink_to_fit(&mut gpa).?;
    if tombstones.len() != 1usize or tombstones.capacity() != 7usize {
        return mem::Error::Invalid!;
    }
    switch tombstones.get(&777) {
        ?value => { if value.* != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrink_to_capacity(&mut gpa, 14usize).?;
    if tombstones.len() != 1usize or tombstones.capacity() != 7usize {
        return mem::Error::Invalid!;
    }
    switch tombstones.remove(&777) {
        ?value => { if value != 888 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tombstones.shrink_to_fit(&mut gpa).?;
    if tombstones.len() != 0usize or tombstones.capacity() != 0usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 10 {
        _ = tombstones.put(&mut gpa, key, key * 11).?;
        key += 1;
    }
    tombstones.reserve(&mut gpa, 64usize).?;
    if tombstones.capacity() < 64usize {
        return mem::Error::Invalid!;
    }
    tombstones.shrink_to_capacity(&mut gpa, 14usize).?;
    if tombstones.len() != 10usize or tombstones.capacity() != 14usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 10 {
        switch tombstones.get(&key) {
            ?value => { if value.* != key * 11 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    let mut fail_storage: [8192]u8 = [0; 8192];
    let mut fail_allocator = FailAllocator::init(&mut fail_storage);
    let mut rollback = std::HashMap[i32, i32]::init_seed(558u64);
    defer rollback.deinit(&mut fail_allocator).?;
    rollback.reserve(&mut fail_allocator, 14usize).?;
    key = 0;
    while key < 14 {
        _ = rollback.put(&mut fail_allocator, key, key * 10).?;
        key += 1;
    }
    fail_allocator.fail_next_alloc();
    switch rollback.put(&mut fail_allocator, 99, 990) {
        !old => { _ = old;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollback.len() != 14usize or rollback.contains_key(&99) {
        return mem::Error::Invalid!;
    }
    fail_allocator.clear_failures();
    key = 0;
    while key < 14 {
        switch rollback.get(&key) {
            ?value => { if value.* != key * 10 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    fail_allocator.fail_next_alloc();
    switch rollback.clone(&mut fail_allocator) {
        !cloned => { _ = cloned;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    fail_allocator.clear_failures();
    fail_allocator.fail_alloc_at = fail_allocator.alloc_count + 2usize;
    switch rollback.clone(&mut fail_allocator) {
        !cloned => { _ = cloned;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollback.len() != 14usize or rollback.contains_key(&99) {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        switch rollback.get(&key) {
            ?value => { if value.* != key * 10 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    fail_allocator.clear_failures();
    _ = rollback.put(&mut fail_allocator, 99, 990).?;
    switch rollback.get(&99) {
        ?value => { if value.* != 990 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut rollback_get = std::HashMap[i32, i32]::init_seed(559u64);
    defer rollback_get.deinit(&mut fail_allocator).?;
    rollback_get.reserve(&mut fail_allocator, 7usize).?;
    key = 0;
    while key < 7 {
        _ = rollback_get.put(&mut fail_allocator, key, key).?;
        key += 1;
    }
    fail_allocator.fail_next_alloc();
    switch rollback_get.get_or_put(&mut fail_allocator, 77) {
        !entry => { _ = entry;
                return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }
    if rollback_get.len() != 7usize or rollback_get.contains_key(&77) {
        return mem::Error::Invalid!;
    }
    fail_allocator.clear_failures();
    let mut rollback_entry = rollback_get.get_or_put(&mut fail_allocator, 77).?;
    if rollback_entry.found_existing() {
        return mem::Error::Invalid!;
    }
    rollback_entry.value().* = 770;
    switch rollback_get.get(&77) {
        ?value => { if value.* != 770 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut free_fail_storage: [8192]u8 = [0; 8192];
    let mut free_fail_allocator = FailAllocator::init(&mut free_fail_storage);
    let mut free_fail = std::HashMap[i32, i32]::init_seed(560u64);
    defer free_fail.deinit(&mut free_fail_allocator).?;
    free_fail.reserve(&mut free_fail_allocator, 14usize).?;
    key = 0;
    while key < 14 {
        _ = free_fail.put(&mut free_fail_allocator, key, key + 5).?;
        key += 1;
    }
    let old_free_fail_capacity = free_fail.capacity();
    free_fail_allocator.fail_next_free();
    switch free_fail.reserve(&mut free_fail_allocator, 64usize) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if free_fail.capacity() <= old_free_fail_capacity or free_fail.len() != 14usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        switch free_fail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    let free_fail_grown_capacity = free_fail.capacity();
    free_fail_allocator.fail_next_free();
    switch free_fail.compact(&mut free_fail_allocator) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if free_fail.capacity() != free_fail_grown_capacity or free_fail.len() != 14usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 14 {
        switch free_fail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    free_fail_allocator.clear_failures();
    free_fail_allocator.fail_next_free();
    switch free_fail.shrink_to_fit(&mut free_fail_allocator) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::Invalid as i32 {
                return err!;
            } },
    }
    if free_fail.capacity() != 14usize or free_fail.len() != 14usize {
        return mem::Error::Invalid!;
    }
    free_fail_allocator.clear_failures();
    free_fail_allocator.fail_next_alloc();
    free_fail.shrink_to_capacity(&mut free_fail_allocator, free_fail.capacity()).?;
    key = 0;
    while key < 14 {
        switch free_fail.get(&key) {
            ?value => { if value.* != key + 5 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    free_fail_allocator.clear_failures();
    _ = free_fail.put(&mut free_fail_allocator, 90, 900).?;
    switch free_fail.get(&90) {
        ?value => { if value.* != 900 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut collisions = std::collections::HashMapWithContext[i32, i32, ConstantHashContext]::init_context_seed(
        ConstantHashContext::init(),
        777u64,
    );
    defer collisions.deinit(&mut gpa).?;
    collisions.reserve(&mut gpa, 16usize).?;
    key = 0;
    while key < 20 {
        _ = collisions.put(&mut gpa, key, key + 1000).?;
        key += 1;
    }
    if collisions.len() != 20usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 20 {
        switch collisions.get(&key) {
            ?value => { if value.* != key + 1000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 0;
    while key < 10 {
        switch collisions.remove(&key) {
            ?value => { if value != key + 1000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    key = 100;
    while key < 110 {
        _ = collisions.put(&mut gpa, key, key + 2000).?;
        key += 1;
    }
    if collisions.len() != 20usize {
        return mem::Error::Invalid!;
    }
    key = 10;
    while key < 20 {
        if not collisions.contains_key(&key) {
            return mem::Error::Invalid!;
        }
        key += 1;
    }
    key = 100;
    while key < 110 {
        switch collisions.get(&key) {
            ?value => { if value.* != key + 2000 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    let mut modulo = std::collections::HashMapWithContext[Key, i32, ModuloContext]::init_context_seed(
        ModuloContext::init(5, 0x9e3779b97f4a7c15u64),
        19u64,
    );
    defer modulo.deinit(&mut gpa).?;
    switch modulo.put(&mut gpa, Key::init(1), 10) {
        !old => { switch old {
         ?value => { return mem::Error::Invalid!; },
         null => { },
     }; },
        err! => { return err!; },
    }
    switch modulo.put(&mut gpa, Key::init(6), 60) {
        !old => { switch old {
         ?value => { if value != 10 {
                        return mem::Error::Invalid!;
                    } },
         null => { return mem::Error::Invalid!; },
     }; },
        err! => { return err!; },
    }
    if modulo.len() != 1usize {
        return mem::Error::Invalid!;
    }
    let equivalent = Key::init(11);
    switch modulo.get_key(&equivalent) {
        ?stored_key => { if stored_key.value != 1 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch modulo.get_key_value(&equivalent) {
        ?entry => { if entry.key().*.value != 1 or entry.value().* != 60 {
                    return mem::Error::Invalid!;
                } },
        null => { return mem::Error::Invalid!; },
    }
    switch modulo.get(&equivalent) {
        ?value => { if value.* != 60 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    key = 20;
    while key < 60 {
        _ = modulo.put(&mut gpa, Key::init(key), key * 3).?;
        key += 1;
    }
    if modulo.len() != 5usize {
        return mem::Error::Invalid!;
    }
    let replaced = Key::init(46);
    switch modulo.get(&replaced) {
        ?value => { if value.* != 56 * 3 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut tail_probe = std::collections::HashMapWithContext[i32, i32, TailHashContext]::init_context_seed(
        TailHashContext::init(15usize),
        0u64,
    );
    defer tail_probe.deinit(&mut gpa).?;
    tail_probe.reserve(&mut gpa, 14usize).?;
    if tail_probe.capacity() != 14usize {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 8 {
        _ = tail_probe.put(&mut gpa, key, key + 100).?;
        key += 1;
    }
    key = 0;
    while key < 8 {
        switch tail_probe.get(&key) {
            ?value => { if value.* != key + 100 {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }
    switch tail_probe.remove(&0) {
        ?value => { if value != 100 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch tail_probe.put(&mut gpa, 16, 1600) {
        !old => { switch old {
         ?value => { return mem::Error::Invalid!; },
         null => { },
     }; },
        err! => { return err!; },
    }
    switch tail_probe.get(&16) {
        ?value => { if value.* != 1600 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    tail_probe.clear();
    _ = tail_probe.put(&mut gpa, 0, 700).?;
    switch tail_probe.get(&0) {
        ?value => { if value.* != 700 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }

    let mut tiny_storage: [16]u8 = [0; 16];
    let mut tiny = mem::FixedBufferAllocator::init(&mut tiny_storage);
    let mut tiny_map = std::HashMap[i32, i32]::init_seed(11u64);
    defer tiny_map.deinit(&mut tiny).?;
    switch tiny_map.reserve(&mut tiny, 64usize) {
        !ok => { return mem::Error::Invalid!; },
        err! => { if err as i32 != mem::Error::OutOfMemory as i32 {
                return err!;
            } },
    }

    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    run(init).exit().?;
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

fn run(init: process::Init) mem::Error!void {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut map = std::HashMap[i32, i32]::init_seed(42u64);
    defer map.deinit(&mut page).?;

    _ = map.put(&mut page, 1, 10).?;
    _ = map.put(&mut page, 2, 20).?;
    debug::print(&"hash_map={}\n", &[&map]);
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    run(init).exit().?;
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(&exe).output_timeout_without_resources("run emitted executable");
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

fn run(init: process::Init) mem::Error!void {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut gpa = mem::GeneralPurposeAllocator::init(&mut page);
    defer gpa.deinit().ok().?;

    let mut model_map = std::HashMap[i32, i32]::init_seed(557u64);
    defer model_map.deinit(&mut gpa).?;
    let mut expected: [40]i32 = [0; 40];
    let mut present: [40]bool = [false; 40];
    let mut expected_len = 0usize;
    let mut step = 0;
    while step < 240 {
        let slot = (step * 17 + 3) % 40;
        let op = step % 6;
        if op == 0 {
            let was_present = present[slot];
            switch model_map.put(&mut gpa, slot, step + 1000) {
                !old => { switch old {
         ?value => { if not was_present or value != expected[slot] {
                                return mem::Error::Invalid!;
                            } },
         null => { if was_present {
                                return mem::Error::Invalid!;
                            } },
     }; },
                err! => { return err!; },
            }
            if not was_present {
                expected_len += 1usize;
            }
            present[slot] = true;
            expected[slot] = step + 1000;
        } else if op == 1 {
            let was_present = present[slot];
            switch model_map.remove(&slot) {
                ?value => { if not was_present or value != expected[slot] {
                            return mem::Error::Invalid!;
                        }
                        present[slot] = false;
                        expected_len -= 1usize; },
                null => { if was_present {
                        return mem::Error::Invalid!;
                    } },
            }
        } else if op == 2 {
            let mut entry = model_map.get_or_put(&mut gpa, slot).?;
            if present[slot] {
                if not entry.found_existing() or entry.value().* != expected[slot] {
                    return mem::Error::Invalid!;
                }
            } else {
                if entry.found_existing() {
                    return mem::Error::Invalid!;
                }
                expected_len += 1usize;
                present[slot] = true;
            }
            entry.value().* = step + 2000;
            expected[slot] = step + 2000;
        } else if op == 3 {
            let was_present = present[slot];
            switch model_map.put_if_absent(&mut gpa, slot, step + 3000) {
                !inserted => { if inserted == was_present {
                            return mem::Error::Invalid!;
                        }
                        if inserted {
                            expected_len += 1usize;
                            present[slot] = true;
                            expected[slot] = step + 3000;
                        } },
                err! => { return err!; },
            }
        } else if op == 4 {
            switch model_map.get(&slot) {
                ?value => { if not present[slot] or value.* != expected[slot] {
                        return mem::Error::Invalid!;
                    } },
                null => { if present[slot] {
                        return mem::Error::Invalid!;
                    } },
            }
        } else {
            let was_present = present[slot];
            switch model_map.fetch_put(&mut gpa, slot, step + 4000) {
                !old => { switch old {
         ?value => { if not was_present or value != expected[slot] {
                                return mem::Error::Invalid!;
                            } },
         null => { if was_present {
                                return mem::Error::Invalid!;
                            } },
     }; },
                err! => { return err!; },
            }
            if not was_present {
                expected_len += 1usize;
            }
            present[slot] = true;
            expected[slot] = step + 4000;
        }

        if model_map.len() != expected_len {
            return mem::Error::Invalid!;
        }
        step += 1;
    }

    let mut model_count = 0usize;
    let mut model_sum = 0;
    for entry in model_map.iter() {
        let entry_key = entry.key().*;
        if entry_key < 0 or entry_key >= 40 {
            return mem::Error::Invalid!;
        }
        if not present[entry_key] or entry.value().* != expected[entry_key] {
            return mem::Error::Invalid!;
        }
        model_count += 1usize;
        model_sum += entry.value().*;
    }
    if model_count != expected_len {
        return mem::Error::Invalid!;
    }

    let mut key = 0;
    let mut expected_sum = 0;
    while key < 40 {
        if present[key] {
            expected_sum += expected[key];
        }
        key += 1;
    }
    if model_sum != expected_sum {
        return mem::Error::Invalid!;
    }

    let mut cloned_model = model_map.clone(&mut gpa).?;
    defer cloned_model.deinit(&mut gpa).?;
    if cloned_model.len() != model_map.len() or cloned_model.capacity() != model_map.capacity() {
        return mem::Error::Invalid!;
    }
    key = 0;
    while key < 40 {
        switch cloned_model.get(&key) {
            ?value => { if not present[key] or value.* != expected[key] {
                    return mem::Error::Invalid!;
                } },
            null => { if present[key] {
                    return mem::Error::Invalid!;
                } },
        }
        key += 1;
    }
    _ = model_map.put(&mut gpa, 3, 12345).?;
    switch cloned_model.get(&3) {
        ?value => { if value.* == 12345 {
                return mem::Error::Invalid!;
            }; },
        null => { },
    }

    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    run(init).exit().?;
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
fn emit_exe_std_hash_map_reserve_exact_compacts_tombstones() {
    let root = temp_dir("emit_exe_std_hash_map_reserve_exact_compacts_tombstones");
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
    alloc_count: usize,
    fail_alloc_at: usize,
}

extend FailAllocator {
    fn init(buffer: &mut [u8]) FailAllocator {
        {
            backing: mem::FixedBufferAllocator::init(buffer),
            alloc_count: 0usize,
            fail_alloc_at: 0usize,
        }
    }

    fn fail_next_alloc(&mut self) void {
        self.fail_alloc_at = self.alloc_count + 1usize;
    }
}

extend FailAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if not layout.is_empty() {
            self.alloc_count += 1usize;
            if self.fail_alloc_at == self.alloc_count {
                return mem::Error::OutOfMemory!;
            }
        }
        self.backing.alloc(layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        self.backing.free(block)
    }

    fn resize(&mut self, block: mem::Block, new_layout: mem::Layout) bool {
        self.backing.resize(block, new_layout)
    }
}

fn run(init: process::Init) mem::Error!void {
    _ = init;
    let mut storage: [32768]u8 = [0; 32768];
    let mut allocator = FailAllocator::init(&mut storage);
    let mut map = std::HashMap[i32, i32]::init_seed(123u64);
    defer map.deinit(&mut allocator).?;

    map.reserve_exact(&mut allocator, 14usize).?;
    let mut key = 0;
    while key < 14 {
        _ = map.put(&mut allocator, key, key).?;
        key += 1;
    }

    key = 0;
    while key < 14 {
        switch map.remove(&key) {
            ?value => { if value != key {
                    return mem::Error::Invalid!;
                } },
            null => { return mem::Error::Invalid!; },
        }
        key += 1;
    }

    map.reserve_exact(&mut allocator, 2usize).?;
    allocator.fail_next_alloc();
    _ = map.put(&mut allocator, 100, 1000).?;
    _ = map.put(&mut allocator, 101, 1010).?;

    if map.len() != 2usize {
        return mem::Error::Invalid!;
    }
    switch map.get(&100) {
        ?value => { if value.* != 1000 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get(&101) {
        ?value => { if value.* != 1010 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    run(init).exit().?;
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

fn run(init: process::Init) mem::Error!void {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let mut map = std::HashMap[i32, i32]::init_seed(321u64);
    defer map.deinit(&mut page).?;

    map.reserve(&mut page, 4usize).?;
    switch map.put_assume_capacity(1, 10) {
        ?old => { return mem::Error::Invalid!; },
        null => { },
    }
    switch map.put_assume_capacity(1, 11) {
        ?old => { if old != 10 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    if map.put_if_absent_assume_capacity(1, 99) {
        return mem::Error::Invalid!;
    }
    if not map.put_if_absent_assume_capacity(2, 20) {
        return mem::Error::Invalid!;
    }

    let mut existing = map.get_or_put_value_assume_capacity(1, 111);
    if not existing.found_existing() or existing.value().* != 11 {
        return mem::Error::Invalid!;
    }
    existing.value().* = 12;

    let mut inserted = map.get_or_put_assume_capacity(3);
    if inserted.found_existing() {
        return mem::Error::Invalid!;
    }
    inserted.value().* = 30;

    if map.len() != 3usize {
        return mem::Error::Invalid!;
    }
    switch map.get(&1) {
        ?value => { if value.* != 12 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get(&2) {
        ?value => { if value.* != 20 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    switch map.get(&3) {
        ?value => { if value.* != 30 {
                return mem::Error::Invalid!;
            } },
        null => { return mem::Error::Invalid!; },
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    run(init).exit().?;
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
