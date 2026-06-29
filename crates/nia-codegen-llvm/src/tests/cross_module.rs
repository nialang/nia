// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_static_aggregate_initializers() {
    let root = temp_dir("emits_static_aggregate_initializers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    x: i32,
    y: i32,
}

static ratio: f64 = 1.5;
static letter: char = 'A';
static xs: [3]i32 = [1, 2, 3];
static ys: [4]u8 = [b'z'; 4];
static zeroes: [8]i32 = [0; 8];
static pair: Pair = { x: 10, y: 20 };

fn main() i32 {
    pair.x + xs[1] + zeroes[0]
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("double 1.500000e+00"));
    assert!(ir.contains("i32 65"));
    assert!(ir.contains("[3 x i32] [i32 1, i32 2, i32 3]"));
    assert!(ir.contains("[4 x i8] c\"zzzz\"") || ir.contains("[4 x i8] [i8 122"));
    assert!(
        ir.contains("__zeroes = constant [8 x i32] zeroinitializer"),
        "{ir}"
    );
    let pair = mangled_symbol(ir, '%', 0, "Pair");
    assert!(ir.contains(&format!("{pair} {{ i32 10, i32 20 }}")));
}

#[test]
fn emits_static_global_address_initializers() {
    let root = temp_dir("emits_static_global_address_initializers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
static mut target: i32 = 1;
static mut values: [4]i32 = [1, 2, 3, 4];
static p: &i32 = &target;
static q: &i32 = &values[1 + 1];

fn main() i32 {
    p.* + q.*
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let target = mangled_symbol(ir, '@', 0, "target");
    let values = mangled_symbol(ir, '@', 0, "values");
    let p = mangled_symbol(ir, '@', 0, "p");
    let q = mangled_symbol(ir, '@', 0, "q");
    assert!(ir.contains(&format!("{target} = global i32 1")));
    assert!(ir.contains(&format!("{p} = constant ptr {target}")));
    assert!(ir.contains(&format!(
        "{q} = constant ptr getelementptr inbounds ([4 x i32], ptr {values}, i64 0, i64 2)"
    )));
}

#[test]
fn emits_cross_module_function_and_global_references() {
    let root = temp_dir("emits_cross_module_function_and_global_references");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module math;
using entry::math;

static imported_ptr: & i32 = & math::base;

fn main() i32 {
    math::add(imported_ptr.*, math::base)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("math.nia"),
        r#"
pub static mut base: i32 = 40;

pub fn add(a: i32, b: i32) i32 {
    a + b
}
"#,
    )
    .expect("write math source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("external global i32"));
    let base = mangled_symbol(&main_ir.ir, '@', 1, "base");
    let add = mangled_symbol(&main_ir.ir, '@', 1, "add");
    assert!(main_ir.ir.contains(&format!("constant ptr {base}")));
    assert!(main_ir.ir.contains(&format!("call i32 {add}")));
}

#[test]
fn emits_cross_module_struct_literals() {
    let root = temp_dir("emits_cross_module_struct_literals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module geom;
using entry::geom;

fn main() i32 {
    let mut p: geom::Point = { x: 40, y: 2 };
    p.x + p.y
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("geom.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
    )
    .expect("write geom source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert_contains_mangled_symbol(&main_ir.ir, '%', 1, "Point");
    assert!(main_ir.ir.contains("store i32 40"));
    assert!(main_ir.ir.contains("store i32 2"));
    assert!(main_ir.ir.contains("ret i32"));
}
