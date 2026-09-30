// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux executable forms: static-pie (the default), static, and dynamic.
//!
//! Every form must run programs whose data holds addresses: a static-pie
//! applies those relocations in its own startup, a dynamic executable leaves
//! them to its interpreter, and a static one has none.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::{path::Path, process::Command};
mod support;
use nia_test_support::{CommandExt, test_dir as temp_dir};

// A function-pointer table and trait-object vtables put addresses in data;
// the program prints a static's address so runs can be compared.
const PROGRAM: &str = r#"
using std::fmt;
using std::io;
using std::process;

trait Shape {
    fn area(&self) i32;
}

struct Square { side: i32 }
struct Rect { w: i32, h: i32 }

extend Square : Shape {
    fn area(&self) i32 { self.side * self.side }
}

extend Rect : Shape {
    fn area(&self) i32 { self.w * self.h }
}

fn one() i32 { 1 }
fn two() i32 { 2 }
fn three() i32 { 3 }

static handlers: [&fn() i32; 3] = [&one, &two, &three];
static marker: i32 = 7;

fn measure(shape: &Shape) i32 {
    shape.area()
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let square = Square { side: 3 };
    let rect = Rect { w: 4, h: 5 };
    let mut sum = measure(&square) + measure(&rect);
    for &handler in &handlers[..] {
        sum += handler();
    }
    if sum != 35 {
        return process::ExitCode(1)!;
    }
    let address = &marker;
    io::debugPrint(&"{:p}\n", &[&address]).?;
    !()
}
"#;

struct Image {
    kind: u16,
    interpreter: bool,
}

// ELF `e_type` and whether any program header is PT_INTERP.
fn inspect(path: &Path) -> Image {
    let bytes = std::fs::read(path).expect("read executable");
    assert_eq!(&bytes[..4], b"\x7fELF");
    let wide = bytes[4] == 2;
    let read = |offset: usize, size: usize| {
        bytes[offset..offset + size]
            .iter()
            .rev()
            .fold(0usize, |value, byte| (value << 8) | usize::from(*byte))
    };
    let (phoff, phentsize, phnum) = if wide {
        (read(32, 8), read(54, 2), read(56, 2))
    } else {
        (read(28, 4), read(42, 2), read(44, 2))
    };
    Image {
        kind: read(16, 2) as u16,
        interpreter: (0..phnum).any(|index| read(phoff + index * phentsize, 4) == 3),
    }
}

fn run(path: &Path) -> String {
    let output = Command::new(path).output_timeout_for_runtime("run executable form");
    assert_eq!(output.status.code(), Some(0), "{}", path.display());
    // `debugPrint` writes to standard error.
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn randomized() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/randomize_va_space")
        .is_ok_and(|value| value.trim() != "0")
}

#[test]
fn linux_executable_forms_run_programs_with_relocated_data() {
    const ET_EXEC: u16 = 2;
    const ET_DYN: u16 = 3;
    let root = temp_dir("linux_executable_forms");
    let main = root.join("main.nia");
    std::fs::write(&main, PROGRAM).expect("write program");
    for target in ["x86_64-unknown-linux", "x86-unknown-linux"] {
        // LLD packs relocations as RELR; GNU ld emits them as RELA or REL.
        for (form, flavor) in [
            ("static-pie", "lld"),
            ("static-pie", "gnu"),
            ("static", "lld"),
            ("dynamic", "lld"),
        ] {
            let case = format!("{target} {form} {flavor}");
            let exe = root.join(format!("{target}-{form}-{flavor}"));
            let output = support::nia_command()
                .args(["--target", target, "emit", "--exe"])
                .arg(&main)
                .args(["--executable", form, "--linker-flavor", flavor, "-o"])
                .arg(&exe)
                .output_timeout_for_build("emit executable form");
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let image = inspect(&exe);
            let expected = match form {
                "static-pie" => (ET_DYN, false),
                "static" => (ET_EXEC, false),
                _ => (ET_DYN, true),
            };
            assert_eq!((image.kind, image.interpreter), expected, "{case}");
            let first = run(&exe);
            if form == "static-pie" && randomized() {
                // The kernel chooses a fresh base for every run.
                let differs = (0..8).any(|_| run(&exe) != first);
                assert!(differs, "{case}: static-pie base never moved from {first}");
            }
        }
    }
}
