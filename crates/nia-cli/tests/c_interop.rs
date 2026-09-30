// SPDX-License-Identifier: GPL-3.0-or-later
//! Nia objects are ordinary platform objects: a C toolchain links them with its
//! own defaults, and only `extern` definitions become part of the result.
#![cfg(target_os = "linux")]

use std::{
    path::{Path, PathBuf},
    process::Command,
};
mod support;
use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

// Trait objects put function addresses in data, which a PIE link must be able
// to relocate at load time without writing to text.
const LIBRARY: &str = r#"
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

fn measure(shape: &Shape) i32 {
    shape.area()
}

static mut calls: i32 = 0;

pub extern fn shapes_area(side: i32, w: i32, h: i32) i32 {
    calls += 1;
    let square = Square { side };
    let rect = Rect { w, h };
    measure(&square) + measure(&rect) + calls
}
"#;

const PROGRAM: &str = r#"
int shapes_area(int, int, int);
int main(void) { return shapes_area(3, 4, 5) == 30 && shapes_area(3, 4, 5) == 31 ? 0 : 1; }
"#;

fn emit_objects(root: &Path, target: &str) -> Vec<PathBuf> {
    let source = root.join("shapes.nia");
    std::fs::write(&source, LIBRARY).expect("write Nia library source");
    let objects = root.join(format!("objects-{target}"));
    let output = support::nia_command()
        .args(["--target", target, "emit", "--obj"])
        .arg(&source)
        .args(["--runtime", "bare", "--out-dir"])
        .arg(&objects)
        .output_timeout_for_build("emit Nia library objects");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut paths = std::fs::read_dir(&objects)
        .expect("read emitted objects")
        .map(|entry| entry.expect("object entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn cc(root: &Path, flags: &[&str], inputs: &[PathBuf], output: &Path) {
    let program = root.join("main.c");
    std::fs::write(&program, PROGRAM).expect("write C program");
    let status = Command::new("cc")
        .args(flags)
        // Relocations against read-only segments are errors, not warnings.
        .arg("-Wl,-z,text")
        .arg(&program)
        .args(inputs)
        .arg("-o")
        .arg(output)
        .status_timeout("link Nia objects with the C toolchain");
    assert!(status.success(), "cc {flags:?} failed: {status}");
}

fn dynamic_exports(library: &Path) -> Vec<String> {
    // The dynamic symbol table is exactly the exported interface.
    let output = Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(library)
        .output_timeout_for_runtime("list shared library exports");
    assert!(output.status.success(), "nm failed");
    let mut exports = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(2))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    exports.sort();
    exports
}

#[test]
fn c_programs_link_nia_objects_with_their_default_pie_toolchain() {
    for (target, flags) in [
        ("x86_64-unknown-linux", &[][..]),
        ("x86-unknown-linux", &["-m32"][..]),
    ] {
        let root = temp_dir(&format!("c_interop_pie_{target}"));
        let objects = emit_objects(&root, target);
        let program = root.join("program");
        cc(&root, flags, &objects, &program);
        let status = Command::new(&program).status_timeout("run C program");
        assert_eq!(status.code(), Some(0), "{target}");
    }
}

#[test]
fn shared_libraries_export_only_extern_definitions() {
    for (target, flags) in [
        ("x86_64-unknown-linux", &["-shared"][..]),
        ("x86-unknown-linux", &["-m32", "-shared"][..]),
    ] {
        let root = temp_dir(&format!("c_interop_shared_{target}"));
        let objects = emit_objects(&root, target);
        let library = root.join("libshapes.so");
        let status = Command::new("cc")
            .args(flags)
            .arg("-Wl,-z,text")
            .args(&objects)
            .arg("-o")
            .arg(&library)
            .status_timeout("link Nia objects as a shared library");
        assert!(status.success(), "{target}: {status}");
        assert_eq!(dynamic_exports(&library), ["shapes_area"], "{target}");
    }
}
