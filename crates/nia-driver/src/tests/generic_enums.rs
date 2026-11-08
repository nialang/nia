// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn generic_enum_payload_and_match_link_and_run() {
    let root = temp_dir("generic_enum_payload_and_match");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
using std::process;

trait Extract[T] {
    fun extract(&self, fallback: T) T;
}

enum Option[T] {
    Some(T),
    None,
}

enum Packet[T, N: usize] {
    Value(T),
    Bytes { data: [u8; N] },
    Nested { option: Option[T], data: [T; N] },
    Empty,
}

extend[T] Option[T] : Extract[T] {
    fun extract(&self, fallback: T) T {
        match self.* {
            Option[T]::Some(value) => value,
            Option[T]::None => fallback,
        }
    }
}

fun read[T](value: Option[T], fallback: T) T where Option[T]: Extract[T] {
    value.extract(fallback)
}

fun packet_value(value: Packet[i32, 3]) i32 {
    match value {
        Packet[i32, 3]::Value(number) => number,
        Packet[i32, 3]::Bytes { data } => data[0] as i32,
        Packet[i32, 3]::Nested { option, data } => read[i32](option, 0) + data[0],
        Packet[i32, 3]::Empty => 0,
    }
}

fun packet_value64(value: Packet[i64, 4]) i64 {
    match value {
        Packet[i64, 4]::Value(number) => number,
        Packet[i64, 4]::Bytes { data } => data[0] as i64,
        Packet[i64, 4]::Nested { option, data } => read[i64](option, 0) + data[0],
        Packet[i64, 4]::Empty => 0,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    let make: &fun(i32) Option[i32] = &Option[i32]::Some;
    let some = read[i32](make(9), 0);
    let make64: &fun(i64) Option[i64] = &Option[i64]::Some;
    let some64 = read[i64](make64(10), 0);
    let none = read[i32](Option[i32]::None, 7);
    let packet_make: &fun(i32) Packet[i32, 3] = &Packet[i32, 3]::Value;
    let packet = packet_value(packet_make(8));
    let bytes_make: &fun([u8; 3]) Packet[i32, 3] = &Packet[i32, 3]::Bytes;
    let bytes = packet_value(bytes_make([4, 5, 6]));
    let nested = packet_value(Packet[i32, 3]::Nested {
        option: Option[i32]::Some(5),
        data: [6, 7, 8],
    });
    let packet_make64: &fun(i64) Packet[i64, 4] = &Packet[i64, 4]::Value;
    let packet64 = packet_value64(packet_make64(12));
    _ = init;
    if some != 9
        or some64 != 10
        or none != 7
        or packet != 8
        or bytes != 4
        or nested != 11
        or packet64 != 12
    {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );

    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("generic enum executable must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run generic enum executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn imported_generic_enum_instances_link_with_distinct_constructor_abis() {
    let root = temp_dir("imported_generic_enum_instances");
    write(
        &root.join("model.nia"),
        r#"
pub enum Packet[T, N: usize] {
    Value(T),
    Bytes { data: [u8; N] },
    Empty,
}
"#,
    );
    let source = root.join("main.nia");
    write(
        &source,
        r#"
module model;
using root::model;
using root::model::Packet;
using std::process;

fun read32(value: Packet[i32, 3], fallback: i32) i32 {
    match value {
        Packet[i32, 3]::Value(number) => number,
        _ => fallback,
    }
}

fun read64(value: Packet[i64, 3], fallback: i64) i64 {
    match value {
        Packet[i64, 3]::Value(number) => number,
        _ => fallback,
    }
}

fun read64_4(value: Packet[i64, 4], fallback: i64) i64 {
    match value {
        Packet[i64, 4]::Value(number) => number,
        _ => fallback,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    let make32: &fun(i32) Packet[i32, 3] = &Packet[i32, 3]::Value;
    let make64: &fun(i64) Packet[i64, 3] = &Packet[i64, 3]::Value;
    let bytes32: &fun([u8; 3]) Packet[i32, 3] = &Packet[i32, 3]::Bytes;
    let bytes64: &fun([u8; 4]) Packet[i64, 4] = &Packet[i64, 4]::Bytes;
    let value32 = read32(make32(21), 0);
    let value64 = read64(make64(34), 0);
    let byte_value32 = read32(bytes32([1, 2, 3]), 0);
    let byte_value64 = read64_4(bytes64([4, 5, 6, 7]), 0);
    _ = init;
    if value32 != 21 or value64 != 34 or byte_value32 != 0 or byte_value64 != 0 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );

    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("imported generic enum executable must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run imported generic enum executable");
    assert_eq!(status.code(), Some(0));
}
