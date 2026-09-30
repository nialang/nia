// SPDX-License-Identifier: GPL-3.0-or-later
//! `std::builtin::size[T]()` is the storage an executable really uses: the
//! distance between neighbouring array elements equals the size for aggregates
//! that hold an 8-byte scalar, whose alignment the target data model decides
//! (4 on i686, 8 elsewhere).

use std::path::Path;
use std::process::Command;
mod support;
use nia_test_support::{CommandExt, test_dir};

const SOURCE: &str = r#"
using std::io;
using std::fmt;
using std::process;
using process::{Init, ExitCode};

struct Wide {
    tag: u8,
    count: i64,
}

extern struct CWide {
    tag: u8,
    ratio: f64,
}

enum Choice {
    Small(u8),
    Large(u64),
}

fn stride(first: usize, second: usize) usize {
    second - first
}

pub fn main(init: Init) ExitCode!() {
    _ = init;
    let wide: [Wide; 2] = [Wide { tag: 1, count: 2 }, Wide { tag: 3, count: 4 }];
    let c_wide: [CWide; 2] = [CWide { tag: 1, ratio: 2.0 }, CWide { tag: 3, ratio: 4.0 }];
    let tuple: [(u8, f64); 2] = [(1, 2.0), (3, 4.0)];
    let choice: [Choice; 2] = [Choice::Small(1), Choice::Large(2)];
    let optional: [?i64; 2] = [null, ?5i64];
    let sizes = [
        std::builtin::size[Wide](),
        std::builtin::size[CWide](),
        std::builtin::size[(u8, f64)](),
        std::builtin::size[Choice](),
        std::builtin::size[?i64](),
    ];
    let strides = [
        stride(&wide[0] as usize, &wide[1] as usize),
        stride(&c_wide[0] as usize, &c_wide[1] as usize),
        stride(&tuple[0] as usize, &tuple[1] as usize),
        stride(&choice[0] as usize, &choice[1] as usize),
        stride(&optional[0] as usize, &optional[1] as usize),
    ];
    for index in 0..5 {
        io::debugPrint(&"{} {}\n", &[&sizes[index], &strides[index]]).?;
    }
    !()
}
"#;

// Every probed aggregate holds one byte and one 8-byte scalar.
fn assert_sizes_are_strides(command: Command, case: &str, expected_size: u64) {
    let mut command = command;
    let output = command.output_timeout_for_runtime("run layout probe");
    assert_eq!(output.status.code(), Some(0), "{case}");
    let text = String::from_utf8_lossy(&output.stderr);
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 5, "{case}: {text}");
    for line in lines {
        let pair = line
            .split(' ')
            .map(|value| value.parse::<u64>().expect("size and stride"))
            .collect::<Vec<_>>();
        assert_eq!(pair, [expected_size, expected_size], "{case}: {text}");
    }
}

fn emit(source: &Path, target: Option<&str>, exe: &Path) {
    let mut command = support::nia_command();
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    let emitted = command
        .args(["emit", "--exe"])
        .arg(source)
        .arg("-o")
        .arg(exe)
        .output_timeout_for_build("emit layout probe");
    assert!(
        emitted.status.success(),
        "{target:?}: {}",
        String::from_utf8_lossy(&emitted.stderr)
    );
}

#[test]
fn sizes_are_array_strides_on_the_host() {
    let root = test_dir("sizes_are_array_strides_on_the_host");
    let source = root.join("main.nia");
    std::fs::write(&source, SOURCE).expect("write layout probe");
    let exe = root
        .join("main")
        .with_extension(std::env::consts::EXE_EXTENSION);
    emit(&source, None, &exe);
    assert_sizes_are_strides(Command::new(&exe), "host", 16);
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn sizes_are_array_strides_on_every_linux_target() {
    let root = test_dir("sizes_are_array_strides_on_every_linux_target");
    let source = root.join("main.nia");
    std::fs::write(&source, SOURCE).expect("write layout probe");
    for target in nia_test_support::LINUX_TARGETS {
        let exe = root.join(target);
        emit(&source, Some(target), &exe);
        let expected_size = if target.starts_with("x86-") { 12 } else { 16 };
        assert_sizes_are_strides(
            nia_test_support::linux_target_command(target, &exe),
            target,
            expected_size,
        );
    }
}
