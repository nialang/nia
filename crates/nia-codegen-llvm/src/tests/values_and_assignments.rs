// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_if_expression_from_function_ir() {
    let root = temp_dir("emits_if_expression_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let mut x = 1;
    if x == 1 { 40 } else { 2 }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("br i1"));
    assert!(ir.contains("fir.bb"));
    assert!(ir.contains("store i32 40"));
    assert!(ir.contains("store i32 2"));
    assert!(!ir.contains("phi i32"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_signed_integer_comparisons_from_operand_type() {
    let root = temp_dir("emits_signed_integer_comparisons_from_operand_type");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let ret: isize = -2isize;
    if ret < 0isize { 0 } else { 1 }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("icmp slt i64"), "{ir}");
    assert!(!ir.contains("icmp ult i64"), "{ir}");
}

#[test]
fn emits_nested_value_function_flow_from_function_ir() {
    let root = temp_dir("emits_nested_value_function_flow_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn take(x: i32, y: i32) i32 {
    x + y
}

fn main(flag: bool) i32 {
    let mut values = [
        if flag { 1 } else { 2 },
        { let mut tmp = 3; tmp },
        switch 1 {
            0 => 4,
            _ => 5,
        },
    ];
    take(values[if flag { 0usize } else { 1usize }], if flag { 10 } else { 20 })
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("fir.tmp"), "{ir}");
    assert!(ir.contains("switch i32"), "{ir}");
    assert!(ir.contains("call i32 @"));
    assert!(!ir.contains("function expression was not lowered"));
}

#[test]
fn emits_deferred_function_body_from_function_ir() {
    let root = temp_dir("emits_deferred_function_body_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main(flag: bool) i32 {
    defer {
        if flag {
            log(1);
        } else {
            switch 2 {
                1 => log(2),
                _ => log(3),
            };
        };
    };
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("defer.entry"), "{ir}");
    assert!(ir.contains("switch i32"), "{ir}");
    assert!(ir.contains("call void @log(i32 1)"));
    assert!(ir.contains("call void @log(i32 2)"));
    assert!(ir.contains("call void @log(i32 3)"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_while_condition_value_flow_from_function_ir() {
    let root = temp_dir("emits_while_condition_value_flow_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let mut i = 0;
    while { i < 2 } {
        i += 1;
    }
    i
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("fir.tmp"), "{ir}");
    assert!(ir.contains("br i1"), "{ir}");
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_plain_local_assignment() {
    let root = temp_dir("emits_plain_local_assignment");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let mut x = 1;
    x = 41;
    x + 1
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("store i32 41"));
    assert!(ir.contains("llvm.sadd.with.overflow.i32"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn scalar_call_assignment_evaluates_rhs_before_place_address() {
    let root = temp_dir("scalar_call_assignment_evaluates_rhs_before_place_address");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn rhs() i32;
extern fn slot() &mut i32;

fn main() i32 {
    slot().* = rhs();
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_substrings_in_order(ir, &["call i32 @rhs()", "call ptr @slot()"]);
}

#[test]
fn emits_index_assignment_through_mutable_slice_call_result() {
    let root = temp_dir("emits_index_assignment_through_mutable_slice_call_result");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn identity(xs: &mut [i32]) &mut [i32] {
    xs
}

fn main(xs: &mut [i32]) i32 {
    identity(xs)[0usize] = 41;
    identity(xs)[0usize] += 1;
    identity(xs)[0usize]
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("store i32 41"), "{ir}");
    assert!(ir.contains("llvm.sadd.with.overflow.i32"), "{ir}");
}

#[test]
fn emits_struct_array_field_index_and_compound_assignment() {
    let root = temp_dir("emits_struct_array_field_index_and_compound_assignment");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
    y: i32,
}

fn main() i32 {
    let mut p = Point { x: 10, y: 20 };
    let mut xs: [3]i32 = [1, 2, 3];
    p.x += xs[1];
    p.x
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("store i32 10"));
    assert!(ir.contains("store i32 2"));
    assert!(ir.contains("llvm.sadd.with.overflow.i32"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_dynamic_index_assignment_into_struct_array_field() {
    let root = temp_dir("emits_dynamic_index_assignment_into_struct_array_field");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct S {
    x: i32,
}

struct T {
    xs: [4]S,
}

extend S {
    fn make(x: i32) S {
        Self { x }
    }
}

fn build() T {
    let mut t = T { xs: [S::make(0); 4] };

    let mut i = 0u16;
    while i < 4u16 {
        t.xs[i as usize] = S::make(i as i32);
        i += 1u16;
    }

    t
}

fn main() i32 {
    let mut t = build();
    t.xs[2].x
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_dynamic_index_assignment_into_struct_array_field_with_constant_rhs() {
    let root = temp_dir("emits_dynamic_index_assignment_into_struct_array_field_with_constant_rhs");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct S {
    x: i32,
}

struct T {
    xs: [4]S,
}

extend S {
    fn make(x: i32) S {
        Self { x }
    }
}

fn build() T {
    let mut t = T { xs: [S::make(0); 4] };

    let mut i = 0u16;
    while i < 4u16 {
        t.xs[i as usize] = S::make(7);
        i += 1u16;
    }

    t
}

fn main() i32 {
    let mut t = build();
    t.xs[2].x
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_dynamic_index_call_from_struct_function_pointer_array_field() {
    let root = temp_dir("emits_dynamic_index_call_from_struct_function_pointer_array_field");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Table {
    fns: [2]&fn(i32) i32,
}

fn add1(x: i32) i32 {
    x + 1
}

fn add2(x: i32) i32 {
    x + 2
}

fn main() i32 {
    let mut table = Table { fns: [& add1, & add2] };
    let mut i: usize = 1;
    table.fns[i](40)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}
