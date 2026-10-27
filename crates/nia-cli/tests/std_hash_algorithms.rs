// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_hash_algorithms_match_reference_vectors_and_streaming() {
    let root = temp_dir("emit_exe_std_hash_algorithms_match_reference_vectors_and_streaming");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::hash;
using std::hash::Hash;
using std::collections;
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let empty: [u8; 0] = [];
    let oneZero: [u8; 1] = [0];
    let twoBytes: [u8; 2] = [0, 1];
    let threeBytes: [u8; 3] = [0, 1, 2];
    if hash::Fnv1a32::hash(&empty[..]) != 0x811c9dc5u32
        or hash::Fnv1a32::hash(&b"a"[..]) != 0xe40c292cu32
        or hash::Fnv1a32::hash(&b"foobar"[..]) != 0xbf9cf968u32
    {
        return process::ExitCode(1)!;
    }
    if hash::Fnv1a64::hash(&empty[..]) != 0xcbf29ce484222325u64
        or hash::Fnv1a64::hash(&b"a"[..]) != 0xaf63dc4c8601ec8cu64
        or hash::Fnv1a64::hash(&b"foobar"[..]) != 0x85944171f73967e8u64
    {
        return process::ExitCode(2)!;
    }
    if hash::XxHash64::hash(0, &empty[..]) != 0xef46db3751d8e999u64
        or hash::XxHash64::hash(0, &b"a"[..]) != 0xd24ec4f1a98c6e5bu64
        or hash::XxHash64::hash(0, &b"abc"[..]) != 0x44bc2cf5ad770999u64
        or hash::XxHash64::hash(0, &b"message digest"[..]) != 0x066ed728fceeb3beu64
        or hash::XxHash64::hash(0, &b"abcdefghijklmnopqrstuvwxyz"[..]) != 0xcfe1f278fa89835cu64
    {
        return process::ExitCode(3)!;
    }
    if hash::SipHash24::hash(
        0x0706050403020100u64,
        0x0f0e0d0c0b0a0908u64,
        &empty[..],
    ) != 0x726fdb47dd0e0e31u64
        or hash::SipHash24::hash(
            0x0706050403020100u64,
            0x0f0e0d0c0b0a0908u64,
            &oneZero[..],
        ) != 0x74f839c593dc67fdu64
        or hash::SipHash24::hash(
            0x0706050403020100u64,
            0x0f0e0d0c0b0a0908u64,
            &twoBytes[..],
        ) != 0x0d6c8009d9a94f5au64
        or hash::SipHash24::hash(
            0x0706050403020100u64,
            0x0f0e0d0c0b0a0908u64,
            &threeBytes[..],
        ) != 0x85676696d7fb7e2du64
    {
        return process::ExitCode(4)!;
    }
    if hash::SipHash13::hash(0, 0, &empty[..]) != 0xd1fba762150c532cu64
        or hash::SipHash13::hash(0, 0, &b"a"[..]) != 0x407448d2b89b1813u64
        or hash::SipHash13::hash(0, 0, &b"abc"[..]) != 0xc03bc3a0042630f2u64
        or hash::SipHash13::hash(0, 0, &b"message digest"[..]) != 0xb2f79a9462cf6700u64
    {
        return process::ExitCode(9)!;
    }

    let mut data: [u8; 513] = [0; 513];
    for index in 0..data.len() {
        data[index] = (index % 251) as u8;
    }
    for length in 0..=data.len() {
        let expectedXx = hash::XxHash64::hash(0x123456789abcdef0u64, &data[0..length]);
        let mut xx = hash::XxHash64::init(0x123456789abcdef0u64);
        let mut offset: usize = 0;
        while offset < length {
            let count = if length - offset < 13 { length - offset } else { 13 };
            xx.update(&data[offset..(offset + count)]);
            offset += count;
        }
        if xx.finish() != expectedXx or xx.finish() != expectedXx {
            return process::ExitCode(10 + length as i32)!;
        }

        let key0 = 0x0706050403020100u64;
        let key1 = 0x0f0e0d0c0b0a0908u64;
        let expected24 = hash::SipHash24::hash(key0, key1, &data[0..length]);
        let mut sip24 = hash::SipHash24::init(key0, key1);
        offset = 0;
        while offset < length {
            let count = if length - offset < 7 { length - offset } else { 7 };
            sip24.update(&data[offset..(offset + count)]);
            offset += count;
        }
        if sip24.finish() != expected24 or sip24.finish() != expected24 {
            return process::ExitCode(6)!;
        }

        let expected13 = hash::SipHash13::hash(key0, key1, &data[0..length]);
        let mut sip13 = hash::SipHash13::init(key0, key1);
        offset = 0;
        while offset < length {
            let count = if length - offset < 9 { length - offset } else { 9 };
            sip13.update(&data[offset..(offset + count)]);
            offset += count;
        }
        if sip13.finish() != expected13 {
            return process::ExitCode(7)!;
        }
    }

    let mut fnv32 = hash::Fnv1a32::init();
    let mut fnv64 = hash::Fnv1a64::init();
    for index in 0..data.len() {
        fnv32.update(&data[index..(index + 1)]);
        fnv64.update(&data[index..(index + 1)]);
    }
    if fnv32.finishValue() != hash::Fnv1a32::hash(&data[..])
        or fnv64.finishValue() != hash::Fnv1a64::hash(&data[..])
    {
        return process::ExitCode(8)!;
    }
    let wide: u128 = 0x0f0e0d0c0b0a09080706050403020100u128;
    let wideBytes: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut wideHasher = hash::Fnv1a64::init();
    wide.hash(&mut wideHasher);
    let negative: i128 = -1;
    let mut signedHasher = hash::Fnv1a64::init();
    negative.hash(&mut signedHasher);
    let allOnes: [u8; 16] = [255; 16];
    if wideHasher.finishValue() != hash::Fnv1a64::hash(&wideBytes[..])
        or signedHasher.finishValue() != hash::Fnv1a64::hash(&allOnes[..])
    {
        return process::ExitCode(11)!;
    }
    let mut allocator = mem::PageAllocator::init();
    let context = collections::SipHashMapContext::fromKeys(
        0x0706050403020100u64,
        0x0f0e0d0c0b0a0908u64,
    );
    let mut map = collections::HashMapWithContext[i32, i32, collections::SipHashMapContext]::initContextSeed(
        context,
        17,
    );
    for key in 0..100 {
        _ = map.insert(&mut allocator, key, key * 3).?;
    }
    for key in 0..100 {
        if map.get(&key) is ?value {
            if value.* != key * 3 { return process::ExitCode(12)!; }
        } else {
            return process::ExitCode(12)!;
        }
    }
    map.deinit(&mut allocator).?;

    if collections::SipHashMapContext::tryInit() is error! {
        return process::ExitCode(13)!;
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
        .output_timeout_for_build("run Nia hash algorithm vectors and streaming checks");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run emitted hash algorithm checks");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn emit_exe_std_sha256_matches_fips_vectors_and_streaming_boundaries() {
    let root = temp_dir("emit_exe_std_sha256_matches_fips_vectors_and_streaming_boundaries");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::crypto::hash;
using std::process;

fun sameBytes(left: &[u8], right: &[u8]) bool {
    if left.len() != right.len() {
        return false;
    }
    for index in 0..left.len() {
        if left[index] != right[index] {
            return false;
        }
    }
    true
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let empty: [u8; 0] = [];
    let emptyDigest: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14,
        0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
        0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c,
        0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
    ];
    let abcDigest: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
        0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];
    let twoBlockDigest: [u8; 32] = [
        0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8,
        0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e, 0x60, 0x39,
        0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67,
        0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb, 0x06, 0xc1,
    ];
    if not sameBytes(&hash::Sha256::digest(&empty[..])[..], &emptyDigest[..]) {
        return process::ExitCode(1)!;
    }
    if not sameBytes(&hash::Sha256::digest(&b"abc"[..])[..], &abcDigest[..]) {
        return process::ExitCode(2)!;
    }
    let twoBlock = &b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"[..];
    if not sameBytes(&hash::Sha256::digest(twoBlock)[..], &twoBlockDigest[..]) {
        return process::ExitCode(3)!;
    }

    let mut data: [u8; 513] = [0; 513];
    for index in 0..data.len() {
        data[index] = (index % 251) as u8;
    }
    for length in 0..=data.len() {
        let expected = hash::Sha256::digest(&data[0..length]);
        let mut hasher = hash::Sha256::init();
        let mut offset: usize = 0;
        while offset < length {
            let chunk = (offset % 73) + 1;
            let count = if length - offset < chunk { length - offset } else { chunk };
            hasher.update(&data[offset..(offset + count)]);
            offset += count;
        }
        if not sameBytes(&hasher.finish()[..], &expected[..])
            or not sameBytes(&hasher.finish()[..], &expected[..])
        {
            return process::ExitCode(4)!;
        }
    }

    let mut continuation = hash::Sha256::init();
    continuation.update(&b"abc"[..]);
    if not sameBytes(&continuation.finish()[..], &abcDigest[..])
        or not sameBytes(&continuation.finish()[..], &abcDigest[..])
    {
        return process::ExitCode(5)!;
    }
    continuation.update(&b"def"[..]);
    if not sameBytes(
        &continuation.finish()[..],
        &hash::Sha256::digest(&b"abcdef"[..])[..],
    ) {
        return process::ExitCode(6)!;
    }

    let block: [u8; 1000] = [0x61; 1000];
    let millionDigest: [u8; 32] = [
        0xcd, 0xc7, 0x6e, 0x5c, 0x99, 0x14, 0xfb, 0x92,
        0x81, 0xa1, 0xc7, 0xe2, 0x84, 0xd7, 0x3e, 0x67,
        0xf1, 0x80, 0x9a, 0x48, 0xa4, 0x97, 0x20, 0x0e,
        0x04, 0x6d, 0x39, 0xcc, 0xc7, 0x11, 0x2c, 0xd0,
    ];
    let mut million = hash::Sha256::init();
    for _ in 0..1000 {
        million.update(&block[..]);
    }
    if not sameBytes(&million.finish()[..], &millionDigest[..]) {
        return process::ExitCode(7)!;
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
        .output_timeout_for_build("run Nia SHA-256 vectors and streaming checks");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run emitted SHA-256 checks");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn emit_exe_std_checksums_match_reference_vectors_and_large_streams() {
    let root = temp_dir("emit_exe_std_checksums_match_reference_vectors_and_large_streams");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::checksum;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let empty: [u8; 0] = [];
    let vector = &b"123456789"[..];
    if checksum::Adler32::hash(&empty[..]) != 1u32
        or checksum::Adler32::hash(vector) != 0x091e01deu32
    {
        return process::ExitCode(1)!;
    }
    if checksum::Crc32::hash(&empty[..]) != 0u32
        or checksum::Crc32::hash(vector) != 0xcbf43926u32
        or checksum::Crc32C::hash(&empty[..]) != 0u32
        or checksum::Crc32C::hash(vector) != 0xe3069283u32
        or checksum::Crc32C::hash(&b"abc"[..]) != 0x364b3fb7u32
    {
        return process::ExitCode(2)!;
    }

    let mut adler = checksum::Adler32::init();
    let mut crc32 = checksum::Crc32::init();
    let mut crc32c = checksum::Crc32C::init();
    adler.update(&vector[0..3]);
    adler.update(&vector[3..6]);
    adler.update(&vector[6..]);
    crc32.update(&vector[0..4]);
    crc32.update(&vector[4..]);
    crc32c.update(&vector[0..1]);
    crc32c.update(&vector[1..8]);
    crc32c.update(&vector[8..]);
    if adler.finish() != 0x091e01deu32
        or crc32.finish() != 0xcbf43926u32
        or crc32c.finish() != 0xe3069283u32
    {
        return process::ExitCode(3)!;
    }

    let mut long: [u8; 6000] = [1; 6000];
    let adlerBlock: [u8; 5553] = [1; 5553];
    if checksum::Adler32::hash(&adlerBlock[..]) != 0x707f15b2u32 {
        return process::ExitCode(5)!;
    }
    let mut streamAdler = checksum::Adler32::init();
    let mut streamCrc32 = checksum::Crc32::init();
    let mut streamCrc32c = checksum::Crc32C::init();
    let mut offset: usize = 0;
    while offset < long.len() {
        let count = if long.len() - offset < 47 { long.len() - offset } else { 47 };
        streamAdler.update(&long[offset..(offset + count)]);
        streamCrc32.update(&long[offset..(offset + count)]);
        streamCrc32c.update(&long[offset..(offset + count)]);
        offset += count;
    }
    if streamAdler.finish() != checksum::Adler32::hash(&long[..])
        or streamCrc32.finish() != checksum::Crc32::hash(&long[..])
        or streamCrc32c.finish() != checksum::Crc32C::hash(&long[..])
    {
        return process::ExitCode(4)!;
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
        .output_timeout_for_build("run Nia checksum vectors and streaming checks");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run emitted checksum checks");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
