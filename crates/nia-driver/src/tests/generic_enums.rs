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
        Packet[i32, 3]::Empty => 0,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    let make: &fun(i32) Option[i32] = &Option[i32]::Some;
    let some = read[i32](make(9), 0);
    let none = read[i32](Option[i32]::None, 7);
    let packet_make: &fun(i32) Packet[i32, 3] = &Packet[i32, 3]::Value;
    let packet = packet_value(packet_make(8));
    let bytes_make: &fun([u8; 3]) Packet[i32, 3] = &Packet[i32, 3]::Bytes;
    let bytes = packet_value(bytes_make([4, 5, 6]));
    _ = init;
    if some != 9 or none != 7 or packet != 8 or bytes != 4 {
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
