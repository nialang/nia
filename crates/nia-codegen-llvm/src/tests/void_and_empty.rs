// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_void_values_and_empty_structs() {
    let root = temp_dir("emits_void_values_and_empty_structs");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

fn sink(p: &void) {}

fn main() i32 {
    var unit: void = {};
    var empty: Empty = {};
    var value: i32 = 7;
    sink(&value as &void);
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define void @"));
    assert!(ir.contains("call void @"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_empty_struct_literals_in_runtime_contexts() {
    let root = temp_dir("emits_empty_struct_literals_in_runtime_contexts");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .defs;

struct LocalEmpty {}

fn take_local(value: LocalEmpty) i32 {
    1
}

fn main() i32 {
    var local: LocalEmpty = {};
    var imported: defs::Empty = {};
    take_local(local) + defs::take(imported)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Empty {}

pub fn take(value: Empty) i32 {
    2
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("__take_local()"), "{}", main_ir.ir);
    assert!(
        main_ir.ir.contains("call i32 @nia__m1__d1__take()"),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_empty_struct_literals_in_return_and_call_contexts() {
    let root = temp_dir("emits_empty_struct_literals_in_return_and_call_contexts");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .defs;

struct LocalEmpty {}

fn make_local() LocalEmpty {
    {}
}

fn take_local(value: LocalEmpty) i32 {
    3
}

fn main() i32 {
    take_local({}) + defs::take({}) + take_local(make_local()) + defs::take(defs::make())
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Empty {}

pub fn make() Empty {
    {}
}

pub fn take(value: Empty) i32 {
    5
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("__take_local()"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("__take()"), "{}", main_ir.ir);
}

#[test]
fn emits_generic_empty_struct_literals_across_modules() {
    let root = temp_dir("emits_generic_empty_struct_literals_across_modules");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .defs;

struct LocalBox[T] {}

fn take_local(value: LocalBox[i32]) i32 {
    7
}

fn main() i32 {
    var local: LocalBox[i32] = {};
    var imported: defs::Box[i32] = {};
    take_local(local) + defs::take(imported) + defs::take({})
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Box[T] {}

pub fn take(value: Box[i32]) i32 {
    11
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("__take_local()"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("__take()"), "{}", main_ir.ir);
}

#[test]
fn emits_discarded_void_calls() {
    let root = temp_dir("emits_discarded_void_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn printf(fmt: &const u8, ...);

fn effect() {}
fn value() i32 { 7 }

fn main() i32 {
    _ = effect();
    _ = printf(c"ok\n");
    _ = value();
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare void @printf"));
    assert!(ir.contains("call void (ptr, ...) @printf"));
    assert!(ir.contains("call void @nia__m0__d1__effect"));
    assert!(ir.contains("call i32 @nia__m0__d2__value"));
}
