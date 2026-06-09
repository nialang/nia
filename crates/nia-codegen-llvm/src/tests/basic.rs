// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn codegen_ice_boundary_converts_panic_to_diagnostic() {
    let output = catch_llvm_codegen_ice(|| panic!("Nia ICE (LLVM): invalid value kind"));

    assert!(output.modules.is_empty());
    assert_eq!(output.diagnostics.len(), 1);
    let diagnostic = &output.diagnostics[0];
    assert_eq!(diagnostic.category, DiagnosticCategory::Internal);
    assert_eq!(diagnostic.code.as_str(), "I0001");
    assert!(diagnostic.summary.contains("invalid value kind"));
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("compiler bug"))
    );
}

#[test]
fn emits_declarations_for_checked_program() {
    let root = temp_dir("emits_declarations_for_checked_program");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &u8) i32;
let hello = c"hello";

extern struct Point {
    x: i32,
    y: i32,
}

extern fn use_point(p: Point) i32;

fn main() i32 {
    var x = 40;
    var y = 2;
    x + y
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("declare i32 @use_point"));
    assert!(ir.contains("@nia__m0__d"));
    assert!(ir.contains("%nia__m0__d"));
    assert!(ir.contains("define i32 @"));
    assert!(ir.contains("alloca i32"));
    assert!(ir.contains("store i32 40"));
    assert!(ir.contains("add i32"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_primitive_vector_param_and_return_types() {
    let root = temp_dir("emits_primitive_vector_param_and_return_types");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn id(v: u8x16) u8x16 {
    v
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define <16 x i8> @"),
        "expected vector return type in IR:\n{ir}"
    );
    assert!(
        ir.contains("<16 x i8> %"),
        "expected vector parameter type in IR:\n{ir}"
    );
}

#[test]
fn emits_splat_builtin() {
    let root = temp_dir("emits_splat_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn make(value: u8) u8x16 {
    @splat[u8x16](value)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define <16 x i8> @"),
        "expected vector return type in IR:\n{ir}"
    );
    assert!(ir.contains("i8 %"), "expected scalar parameter:\n{ir}");
    assert!(
        ir.contains("shufflevector"),
        "expected splat shuffle:\n{ir}"
    );
    assert!(ir.contains("<16 x i32>"), "expected vector mask:\n{ir}");
}

#[test]
fn emits_vector_lane_builtins() {
    let root = temp_dir("emits_vector_lane_builtins");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn lane(v: u8x16, i: usize) u8 {
    @extract(v, i)
}

fn changed(v: u8x16, i: usize, x: u8) u8x16 {
    @insert(v, i, x)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("extractelement <16 x i8>"),
        "expected vector lane extract:\n{ir}"
    );
    assert!(
        ir.contains("insertelement <16 x i8>"),
        "expected vector lane insert:\n{ir}"
    );
}

#[test]
fn emits_vector_builtin_operators() {
    let root = temp_dir("emits_vector_builtin_operators");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add_u8(lhs: u8x16, rhs: u8x16) u8x16 {
    lhs + rhs
}

fn and_mask(lhs: boolx16, rhs: boolx16) boolx16 {
    lhs & rhs
}

fn cmp_f32(lhs: f32x4, rhs: f32x4) boolx4 {
    lhs < rhs
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("add <16 x i8>"), "expected vector add:\n{ir}");
    assert!(ir.contains("and <16 x i1>"), "expected vector and:\n{ir}");
    assert!(
        ir.contains("fcmp olt <4 x float>"),
        "expected vector float compare:\n{ir}"
    );
    assert!(
        ir.contains("define <4 x i1> @"),
        "expected vector comparison mask return:\n{ir}"
    );
}

#[test]
fn emits_vector_bitmask_builtin() {
    let root = temp_dir("emits_vector_bitmask_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn matching(v: u8x16, tag: u8) usize {
    @bitmask(v == @splat[u8x16](tag))
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("icmp eq <16 x i8>"),
        "expected vector integer compare:\n{ir}"
    );
    assert!(
        ir.contains("bitcast <16 x i1>"),
        "expected vector mask bitcast:\n{ir}"
    );
    assert!(
        ir.contains("zext i16"),
        "expected mask widening to usize:\n{ir}"
    );
}
