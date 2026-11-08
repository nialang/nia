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

pub fun main(init: process::Init) process::ExitCode!() {
    let some = read[i32](Option[i32]::Some(9), 0);
    let none = read[i32](Option[i32]::None, 7);
    _ = init;
    if some != 9 or none != 7 {
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
