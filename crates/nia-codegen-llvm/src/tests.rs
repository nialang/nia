// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendLayouts, BackendModule, BackendProgram, BackendStruct,
    TypedBody, TypedExpr, TypedExprKind, TypedLocal, TypedLocalKind,
};
use nia_ids::{DefId, GlobalDefId, LocalId, ModuleId};
use nia_layout::{FieldLayout, StructLayout, TypeLayout};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

#[test]
fn codegen_ice_boundary_converts_panic_to_diagnostic() {
    let output = catch_llvm_codegen_ice(|| panic!("Nia ICE (LLVM): invalid value kind"));

    assert!(output.modules.is_empty());
    assert_eq!(output.diagnostics.len(), 1);
    assert!(
        output.diagnostics[0]
            .message
            .contains("internal compiler error: invalid value kind"),
        "{:?}",
        output.diagnostics
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
const hello = c"hello";

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
fn checks_hosted_main_signatures() {
    let root = temp_dir("checks_hosted_main_signatures");
    let good = root.join("good.nia");
    std::fs::write(
        &good,
        r#"
fn main(argc: i32, argv: &const &const u8) i32 {
    argc
}
"#,
    )
    .expect("write good source");

    let checked = nia_driver::check_program(good.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir_with_options(
        &checked.backend_lowering.program,
        LlvmCodegenOptions {
            root_module: Some(ModuleId(0)),
            hosted_entry: true,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("define i32 @main(i32"));

    let bad = root.join("bad.nia");
    std::fs::write(
        &bad,
        r#"
fn main(flag: bool) i32 {
    0
}
"#,
    )
    .expect("write bad source");

    let checked = nia_driver::check_program(bad.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir_with_options(
        &checked.backend_lowering.program,
        LlvmCodegenOptions {
            root_module: Some(ModuleId(0)),
            hosted_entry: true,
        },
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("hosted entry `main`")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn emits_literals_with_expected_context_types() {
    let root = temp_dir("emits_literals_with_expected_context_types");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn take_byte(x: u8) u8 { x }
fn take32(x: f32) f32 { x }

fn main() i32 {
    var default_int = 10;
    var default_float = 1.5;
    var typed_byte: u8 = 11;
    var typed_float: f32 = 3.5;
    var byte: u8 = 10;
    var signed: i8 = -1;
    var float32: f32 = 2.5;
    _ = take_byte(3);
    _ = take32(typed_float);
    default_int + byte as i32 + signed as i32 + typed_byte as i32 + default_float as i32 + float32 as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("store i32 10"));
    assert!(ir.contains("store double"));
    assert!(ir.contains("store i8 11"));
    assert!(ir.contains("store i8 10"));
    assert!(ir.contains("store i8 -1"));
    assert!(ir.contains("store float"));
}

#[test]
fn emits_static_function_pointer_in_struct_initializer() {
    let root = temp_dir("emits_static_function_pointer_in_struct_initializer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Vtable {
    print: &const fn(&i32)
}

fn print_i32(value: &i32) {}

const vtable: Vtable = { print: &const print_i32 };

fn main() i32 {
    var x = 1;
    vtable.print(&x);
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
    assert!(ir.contains("print_i32"), "{ir}");
    assert!(ir.contains("call void"), "{ir}");
}

#[test]
fn rejects_field_access_with_mismatched_base_struct() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let point_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(0),
    };
    let other_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let point_x = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(2),
    };
    let other_y = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(3),
    };
    let point_ty = interner.intern(TyKind::Nominal {
        def_id: point_id,
        args: Vec::new(),
    });
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: nia_comptime_check::ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: vec![
                    (
                        point_id,
                        StructLayout {
                            layout: TypeLayout { size: 4, align: 4 },
                            fields: vec![FieldLayout {
                                def_id: point_x.def_id,
                                offset: 0,
                                layout: TypeLayout { size: 4, align: 4 },
                            }],
                        },
                    ),
                    (
                        other_id,
                        StructLayout {
                            layout: TypeLayout { size: 4, align: 4 },
                            fields: vec![FieldLayout {
                                def_id: other_y.def_id,
                                offset: 0,
                                layout: TypeLayout { size: 4, align: 4 },
                            }],
                        },
                    ),
                ],
                struct_instances: Vec::new(),
                unions: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![
                BackendStruct {
                    def_id: point_id,
                    name: "Point".to_string(),
                    generics: Vec::new(),
                    fields: vec![BackendField {
                        def_id: point_x,
                        name: "x".to_string(),
                        ty: i32_ty,
                        span: Span::default(),
                    }],
                    is_extern: false,
                    span: Span::default(),
                },
                BackendStruct {
                    def_id: other_id,
                    name: "Other".to_string(),
                    generics: Vec::new(),
                    fields: vec![BackendField {
                        def_id: other_y,
                        name: "y".to_string(),
                        ty: i32_ty,
                        span: Span::default(),
                    }],
                    is_extern: false,
                    span: Span::default(),
                },
            ],
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(4),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                body: Some(TypedBody {
                    span: Span::default(),
                    locals: vec![TypedLocal {
                        id: LocalId(0),
                        name: "point".to_string(),
                        kind: TypedLocalKind::Binding,
                        ty: point_ty,
                        span: Span::default(),
                    }],
                    stmts: Vec::new(),
                    tail: Some(Box::new(TypedExpr {
                        span: Span::default(),
                        ty: i32_ty,
                        kind: TypedExprKind::Field {
                            lhs: Box::new(TypedExpr {
                                span: Span::default(),
                                ty: point_ty,
                                kind: TypedExprKind::Local(LocalId(0)),
                            }),
                            field: other_y,
                        },
                    })),
                    ty: i32_ty,
                }),
                span: Span::default(),
            }],
            function_instances: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("missing struct field layout index")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn emits_direct_function_calls() {
    let root = temp_dir("emits_direct_function_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(20, 22)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @nia__m0__d"));
    assert!(ir.contains("call i32 @nia__m0__d"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_extern_function_definitions_with_unmangled_symbols() {
    let root = temp_dir("emits_extern_function_definitions_with_unmangled_symbols");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(40, 2)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @add("), "{ir}");
    assert!(!ir.contains("nia__m0__d0__add"), "{ir}");
    assert!(ir.contains("call i32 @add"), "{ir}");
}

#[test]
fn emits_slice_construction_len_ptr_and_indexing() {
    let root = temp_dir("emits_slice_construction_len_ptr_and_indexing");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: &const [i32]) i32 {
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = &const xs[1..=2];
    var p = @ptr(s);
    var single = &const p[..];
    first(s) + @len(s) as i32 + @len(single) as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("insertvalue"));
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_array_to_slice_coercions() {
    let root = temp_dir("emits_array_to_slice_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: &const [i32]) i32 {
    xs[0]
}

fn first_byte(xs: &const [u8]) i32 {
    xs[0] as i32
}

fn overwrite(xs: &[i32]) i32 {
    xs[1] = 9;
    xs[1]
}

fn main() i32 {
    var xs: [3]i32 = [1, 2, 3];
    var borrow = &const xs[..];
    var literal: &const [i32] = [4, 5, 6];
    first(borrow) + first(literal) + first([7, 8]) + first_byte(c"hi") + overwrite([6, 7])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("arraytmp"), "{ir}");
    assert!(ir.contains("insertvalue"), "{ir}");
    assert!(ir.contains("getelementptr"), "{ir}");
}

#[test]
fn emits_global_string_pointer_call() {
    let root = temp_dir("emits_global_string_pointer_call");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &const u8) i32;
const hello = c"hello";

fn main() i32 {
    _ = puts(&const hello[0]);
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
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("c\"hello\\00\""));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("@nia__m0__d"));
}

#[test]
fn emits_c_string_literal_pointer_coercions() {
    let root = temp_dir("emits_c_string_literal_pointer_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &const u8) i32;

fn first(ptr: &u8) i32 {
    ptr.* = b'J';
    ptr.* as i32
}

fn main() i32 {
    var direct: &const u8 = c"hello";
    var writable: &u8 = c"mutable";
    _ = puts(c"world");
    _ = puts(
        c\\multi
        \\line
    );
    first(writable) + direct.* as i32
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
    assert!(ir.contains("arraytmp"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("[6 x i8] c\"hello\\00\""), "{ir}");
    assert!(ir.contains("[8 x i8] c\"mutable\\00\""), "{ir}");
}

#[test]
fn emits_if_expression_with_phi() {
    let root = temp_dir("emits_if_expression_with_phi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var x = 1;
    if x == 1 { 40 } else { 2 }
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("br i1"));
    assert!(ir.contains("if.then"));
    assert!(ir.contains("if.else"));
    assert!(ir.contains("phi i32"));
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
    var x = 1;
    x = 41;
    x + 1
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("store i32 41"));
    assert!(ir.contains("add i32"));
    assert!(ir.contains("ret i32"));
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
    var p: Point = { x: 10, y: 20 };
    var xs: [3]i32 = [1, 2, 3];
    p.x += xs[1];
    p.x
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("store i32 10"));
    assert!(ir.contains("store i32 2"));
    assert!(ir.contains("add i32"));
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
        { x: x }
    }
}

fn build() T {
    var t: T = { xs: [S::make(0); 4] };

    for var i: u16 = 0; i < 4; i += 1 {
        t.xs[i as usize] = S::make(i as i32);
    }

    t
}

fn main() i32 {
    var t = build();
    t.xs[2].x
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
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
        { x: x }
    }
}

fn build() T {
    var t: T = { xs: [S::make(0); 4] };

    for var i: u16 = 0; i < 4; i += 1 {
        t.xs[i as usize] = S::make(7);
    }

    t
}

fn main() i32 {
    var t = build();
    t.xs[2].x
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
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
    fns: [2]&const fn(i32) i32,
}

fn add1(x: i32) i32 {
    x + 1
}

fn add2(x: i32) i32 {
    x + 2
}

fn main() i32 {
    var table: Table = { fns: [&const add1, &const add2] };
    var i: usize = 1;
    table.fns[i](40)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_nia_structs_in_physical_layout_order() {
    let root = temp_dir("emits_nia_structs_in_physical_layout_order");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Mixed {
    a: u8,
    b: i64,
    c: u8,
}

const mixed: Mixed = { a: 1, b: 2, c: 3 };

fn main() i32 {
    var local: Mixed = { a: 4, b: 5, c: 6 };
    mixed.a as i32 + mixed.c as i32 + local.a as i32 + local.c as i32 + local.b as i32
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
        ir.contains("%nia__m0__d0__Mixed = type { i64, i8, i8 }"),
        "{ir}"
    );
    assert!(
        ir.contains("@nia__m0__d4__mixed = constant %nia__m0__d0__Mixed { i64 2, i8 1, i8 3 }"),
        "{ir}"
    );
    assert!(ir.contains("ptr @nia__m0__d4__mixed, i32 0, i32 1"), "{ir}");
    assert!(ir.contains("ptr %local, i32 0, i32 2"), "{ir}");
    assert!(ir.contains("ptr %local, i32 0, i32 0"), "{ir}");
}

#[test]
fn emits_nia_function_aggregate_parameter_and_return_abi() {
    let root = temp_dir("emits_nia_function_aggregate_parameter_and_return_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn id(pair: Pair) Pair {
    pair
}

fn sum(pair: Pair) i32 {
    pair.a + pair.b
}

fn main() i32 {
    var pair: Pair = { a: 10, b: 20 };
    var copied = id(pair);
    sum(copied)
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
        ir.contains("define void @nia__m0__d3__id(ptr %0, ptr %1)"),
        "{ir}"
    );
    assert!(ir.contains("define i32 @nia__m0__d4__sum(ptr %0)"), "{ir}");
    assert!(
        ir.contains("call void @nia__m0__d3__id(ptr %call.out, ptr %pair"),
        "{ir}"
    );
    assert!(
        ir.contains("call i32 @nia__m0__d4__sum(ptr %copied"),
        "{ir}"
    );
}

#[test]
fn emits_unary_cast_float_and_enum_values() {
    let root = temp_dir("emits_unary_cast_float_and_enum_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Color: u8 {
    Red,
    Blue = 3,
}

fn main(ptr: &const i32) i32 {
    var x = -1;
    var y = !false;
    var n = 1;
    var z: f64 = n as f64;
    var w: i32 = z as i32;
    var addr = ptr as usize;
    var again = addr as &i32;
    x + w + again.* + Color::Blue as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("sitofp"));
    assert!(ir.contains("fptosi"));
    assert!(ir.contains("ptrtoint"));
    assert!(ir.contains("inttoptr"));
    assert!(ir.contains("ret i32"));
    assert!(ir.contains("store i1 true") || ir.contains("store i1 -2"));
}

#[test]
fn emits_local_string_char_and_byte_char_literals() {
    let root = temp_dir("emits_local_string_char_and_byte_char_literals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var text = "A\0";
    var bytes = b"A\0";
    var cstr = c"A";
    var ch = 'A';
    var byte = b'A';
    _ = bytes;
    _ = cstr;
    text[0] as u32 as i32 + ch as u32 as i32 + byte as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("[2 x i32]"));
    assert!(ir.contains("[2 x i8] c\"A\\00\"") || ir.contains("[2 x i8] [i8 65, i8 0]"));
    assert!(ir.contains("store i32 65"));
    assert!(ir.contains("store i8 65"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_adjacent_string_literal_concatenation() {
    let root = temp_dir("emits_adjacent_string_literal_concatenation");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn printf(fmt: &const u8, ...);

const fmt =
    c""
    c"  #  Type      Offset             VirtAddr           FileSiz"
    c""
    c"            MemSiz"
    c""
    c"             Flags Align\n";

fn main() i32 {
    var text = "中" "" "a" "" "b" "" "c" "";
    var bytes = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
    _ = text;
    _ = bytes;
    printf(
        c""
        c"  #  Type      Offset             VirtAddr           FileSiz"
        c""
        c"            MemSiz"
        c""
        c"             Flags Align\n"
    );
    printf(&const fmt[0]);
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
    assert!(ir.contains("[4 x i32]"), "{ir}");
    assert!(
        ir.contains("[4 x i8] c\"nia\\00\"")
            || ir.contains("[4 x i8] [i8 110, i8 105, i8 97, i8 0]"),
        "{ir}"
    );
    assert!(ir.contains("MemSiz"));
    assert!(ir.contains("Flags Align\\0A\\00"));
    assert!(ir.contains("@printf"));
}

#[test]
fn emits_receiver_method_calls() {
    let root = temp_dir("emits_receiver_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Cell {
    value: i32,
}

extend Cell {
    fn get(&const self) i32 {
        self.value
    }

    fn set(&self, value: i32) {
        self.value = value;
    }
}

fn main() i32 {
    var cell: Cell = { value: 1 };
    cell.set(42);
    cell.get()
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
    assert!(ir.contains("call i32 @"));
    assert!(ir.contains("i32 42"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_generic_struct_associated_function_instances() {
    let root = temp_dir("emits_generic_struct_associated_function_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

fn main() i32 {
    var b = Box[i32]::make(42);
    b.value
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%nia__m0__d0__Box__inst__t_i32"));
    assert!(ir.contains("@nia__m0__d2__make__inst__i32"));
    assert!(ir.contains("call void @nia__m0__d2__make__inst__i32(ptr %call.out, i32 42"));
    assert!(ir.contains("define void @nia__m0__d2__make__inst__i32(ptr %0, i32 %1)"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_structural_extension_method_calls() {
    let root = temp_dir("emits_structural_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
type Ptr[T] = &T;

extend i32 {
    fn is_zero(self) bool {
        self == 0
    }
}

extend[T] Ptr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] &const [T] {
    fn size(self) usize {
        @len(self)
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

extend &const fn(i32) i32 {
    fn apply(self, value: i32) i32 {
        self(value)
    }
}

fn inc(value: i32) i32 {
    value + 1
}

fn main(ptr: &i32, xs: &const [i32], triple: [3]i32) i32 {
    if 0.is_zero() {}
    if ptr.is_null() {}
    xs.size() as i32 + triple.first() + (&const inc).apply(1)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d1__is_zero"));
    assert!(ir.contains("@nia__m0__d2__is_null__inst__i32"));
    assert!(ir.contains("@nia__m0__d3__size__inst__i32"));
    assert!(ir.contains("@nia__m0__d4__first__inst__i32"));
    assert!(ir.contains("@nia__m0__d5__apply"));
    assert!(ir.contains("call i1 @nia__m0__d1__is_zero"));
    assert!(ir.contains("call i1 @nia__m0__d2__is_null__inst__i32"));
    assert!(ir.contains("call i64 @nia__m0__d3__size__inst__i32"));
    assert!(ir.contains("call i32 @nia__m0__d4__first__inst__i32"));
    assert!(ir.contains("call i32 @nia__m0__d5__apply"));
}

#[test]
fn emits_imported_generic_structural_extension_method_calls() {
    let root = temp_dir("emits_imported_generic_structural_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .share;

extern fn read_const() &const u8;

fn main(mut_ptr: &u8) i32 {
    var const_ptr = read_const();
    if mut_ptr.null() {
        return 1;
    }
    if const_ptr.null() {
        return 2;
    }
    0
}
"#,
    )
    .expect("write main source");
    std::fs::write(root.join("share.nia"), "import .ptr;").expect("write share source");
    std::fs::write(
        root.join("ptr.nia"),
        r#"
extend[T] &const T {
    pub fn null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn null(self) bool {
        self as usize == 0
    }
}
"#,
    )
    .expect("write ptr source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("__null__inst__u8"), "{ir}");
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("define i1 @"), "{ir}");
}

#[test]
fn emits_specialized_associated_extension_function_calls() {
    let root = temp_dir("emits_specialized_associated_extension_function_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

extend Box[i32] {
    fn make(value: i32) Box[i32] {
        { value: value + 1 }
    }
}

fn main() i32 {
    var a = Box[i32]::make(41);
    var b = Box[bool]::make(true);
    if b.value { a.value } else { 0 }
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @nia__m0__d3__make(ptr %call.out, i32 41"));
    assert!(ir.contains("call void @nia__m0__d2__make__inst__bool(ptr %call.out1, i1 true"));
}

#[test]
fn emits_associated_method_function_pointers() {
    let root = temp_dir("emits_associated_method_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn get(&const self) i32 {
        self.x
    }
}

fn apply(p: &const Point, f: &const fn(&const Point) i32) i32 {
    f(p)
}

fn main() i32 {
    var p: Point = { x: 42 };
    apply(&const p, &const Point::get)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d2__get"), "{ir}");
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_generic_associated_method_function_pointers() {
    let root = temp_dir("emits_generic_associated_method_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&const self) T {
        self.value
    }
}

fn apply(box: &const Box[i32], f: &const fn(&const Box[i32]) i32) i32 {
    f(box)
}

fn main() i32 {
    var box: Box[i32] = { value: 42 };
    apply(&const box, &const Box[i32]::get)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d2__get__inst__i32"), "{ir}");
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_static_associated_method_function_pointer_initializers() {
    let root = temp_dir("emits_static_associated_method_function_pointer_initializers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn get(&const self) i32 {
        self.x
    }
}

const get_ptr: &const fn(&const Point) i32 = &const Point::get;

fn main(p: &const Point) i32 {
    get_ptr(p)
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
        ir.contains("@nia__m0__d3__get_ptr = constant ptr @nia__m0__d2__get"),
        "{ir}"
    );
}

#[test]
fn emits_structural_associated_calls_and_function_pointers() {
    let root = temp_dir("emits_structural_associated_calls_and_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend[T] &T {
    fn null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    var null: &const fn(&u8) bool = &const [&u8]::null;
    var zero: &const fn() usize = &const [&u8]::zero;
    if null(ptr) {}
    if [&u8]::null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("__null__inst__u8"), "{ir}");
    assert!(ir.contains("__zero__inst__u8"), "{ir}");
    assert!(ir.contains("__first__inst__i32"), "{ir}");
    assert!(ir.contains("call i1 %"), "{ir}");
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("call i64 %"), "{ir}");
}

#[test]
fn emits_deep_pointer_structural_associated_calls_and_function_pointers() {
    let root = temp_dir("emits_deep_pointer_structural_associated_calls_and_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend &&&&&&const &&i32 {
    fn null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&&const &&i32) bool {
    var null: &const fn(&&&&&&const &&i32) bool = &const [&&&&&&const &&i32]::null;
    null(ptr) and [&&&&&&const &&i32]::null(ptr)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d0__null"), "{ir}");
    assert!(ir.contains("call i1 %"), "{ir}");
    assert!(ir.contains("call i1 @"), "{ir}");
}

#[test]
fn emits_numeric_literal_suffix_extension_method_calls() {
    let root = temp_dir("emits_numeric_literal_suffix_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend usize {
    fn plus_one(self) usize {
        self + 1usize
    }
}

fn main() usize {
    10usize.plus_one()
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d0__plus_one"));
    assert!(ir.contains("call i64 @nia__m0__d0__plus_one(i64 10"));
}

#[test]
fn emits_short_circuit_logical_operators() {
    let root = temp_dir("emits_short_circuit_logical_operators");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(a: bool, b: bool) i32 {
    var x = a and b;
    var y = a or b;
    if x or y { 1 } else { 0 }
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("logic.rhs"));
    assert!(ir.contains("logic.end"));
    assert!(ir.contains("phi i1"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_break_and_continue() {
    let root = temp_dir("emits_for_break_and_continue");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var sum = 0;
    for var i = 0; i < 10; i += 1 {
        if i == 3 {
            continue;
        }
        if i == 8 {
            break;
        }
        sum += i;
    }
    sum
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("for.cond"));
    assert!(ir.contains("for.body"));
    assert!(ir.contains("for.step"));
    assert!(ir.contains("for.end"));
    assert!(ir.contains("br label"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_defer_before_return_and_block_exit() {
    let root = temp_dir("emits_defer_before_return_and_block_exit");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn inner(x: i32) i32 {
    defer log(10);
    if x == 0 {
        defer log(11);
        return 1;
    }
    {
        defer log(12);
        x + 1
    }
}

fn main() i32 {
    inner(0)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_substrings_in_order(ir, &["call void @log(i32 11)", "call void @log(i32 10)"]);
    assert!(ir.contains("call void @log(i32 12)") || ir.contains("call void @log(i32 12,"));
    assert!(ir.contains("ret i32 1"));
}

#[test]
fn emits_defer_registered_after_earlier_return_branch() {
    let root = temp_dir("emits_defer_registered_after_earlier_return_branch");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn fclose(file: &void) i32;
extern fn fopen(path: &const u8, mode: &const u8) &void;

fn inspect(path: &const u8) i32 {
    var file = fopen(path, c"rb");

    if file as usize == 0 {
        return 1;
    }

    defer {
        _ = fclose(file);
    };

    0
}

fn main(argc: i32, argv: &const &const u8) i32 {
    inspect(argv[0])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call ptr @fopen"));
    assert!(ir.contains("call i32 @fclose"));
    assert!(ir.contains("ret i32 1"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_deferred_block_cleanup() {
    let root = temp_dir("emits_deferred_block_cleanup");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn cleanup() void {}

fn main() i32 {
    defer {
        cleanup();
    };
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
    assert_substrings_in_order(ir, &["call void @nia__m0__d0__cleanup()", "ret i32 0"]);
}

#[test]
fn emits_defer_lifo_for_normal_nested_block_exit() {
    let root = temp_dir("emits_defer_lifo_for_normal_nested_block_exit");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    defer log(1);
    defer log(2);
    {
        defer log(3);
        defer log(4);
    }
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
    assert_substrings_in_order(
        ir,
        &[
            "call void @log(i32 4)",
            "call void @log(i32 3)",
            "call void @log(i32 2)",
            "call void @log(i32 1)",
        ],
    );
}

#[test]
fn emits_defer_before_loop_break_and_continue() {
    let root = temp_dir("emits_defer_before_loop_break_and_continue");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    var i = 0;
    for {
        defer log(20);
        if i == 0 {
            defer log(21);
            i = 1;
            continue;
        }
        defer log(22);
        break;
    }
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
    assert_substrings_in_order(ir, &["call void @log(i32 21)", "call void @log(i32 20)"]);
    assert!(ir.contains("call void @log(i32 22)"));
    assert!(ir.contains("br label"));
}

#[test]
fn emits_switch_statements() {
    let root = temp_dir("emits_switch_statements");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Color: u8 {
    Red,
    Green,
    Blue,
}

fn main(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        Color::Green => {
            return 2;
        }
        _ => return 3,
    }
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
    assert!(ir.contains("switch i8"));
    assert!(ir.contains("switch.arm.0"));
    assert!(ir.contains("switch.default"));
    assert!(ir.contains("ret i32 1"));
    assert!(ir.contains("ret i32 2"));
    assert!(ir.contains("ret i32 3"));
}

#[test]
fn emits_switch_expressions() {
    let root = temp_dir("emits_switch_expressions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn pick(x: u32) i32 {
    switch x {
        0 => 10,
        1 => 20,
        _ => 30,
    }
}

fn early(x: u32) i32 {
    switch x {
        0 => return 1,
        _ => 2,
    }
}

fn main() i32 {
    pick(1) + early(2)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("switch i32"));
    assert!(ir.contains("switchtmp"));
    assert!(ir.contains("phi i32"));
    assert!(ir.contains("ret i32 1"));
}

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

const ratio: f64 = 1.5;
const letter: char = 'A';
const xs: [3]i32 = [1, 2, 3];
const ys: [4]u8 = [b'z'; 4];
const pair: Pair = { x: 10, y: 20 };

fn main() i32 {
    pair.x + xs[1]
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("double 1.500000e+00"));
    assert!(ir.contains("i32 65"));
    assert!(ir.contains("[3 x i32] [i32 1, i32 2, i32 3]"));
    assert!(ir.contains("[4 x i8] c\"zzzz\"") || ir.contains("[4 x i8] [i8 122"));
    assert!(ir.contains("%nia__m0__d0__Pair { i32 10, i32 20 }"));
}

#[test]
fn emits_static_global_address_initializers() {
    let root = temp_dir("emits_static_global_address_initializers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
var target: i32 = 1;
const p: &i32 = &target;

fn main() i32 {
    p.*
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("@nia__m0__d0__target = global i32 1"));
    assert!(ir.contains("@nia__m0__d1__p = constant ptr @nia__m0__d0__target"));
}

#[test]
fn emits_cross_module_function_and_global_references() {
    let root = temp_dir("emits_cross_module_function_and_global_references");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .math;

const imported_ptr: &const i32 = &const math::base;

fn main() i32 {
    math::add(imported_ptr.*, math::base)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("math.nia"),
        r#"
pub var base: i32 = 40;

pub fn add(a: i32, b: i32) i32 {
    a + b
}
"#,
    )
    .expect("write math source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("external global i32"));
    assert!(main_ir.ir.contains("constant ptr @nia__m1__d0__base"));
    assert!(main_ir.ir.contains("call i32 @nia__m1__d1__add"));
}

#[test]
fn emits_cross_module_struct_literals() {
    let root = temp_dir("emits_cross_module_struct_literals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .geom;

fn main() i32 {
    var p: geom::Point = { x: 40, y: 2 };
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

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("%nia__m1__d0__Point"));
    assert!(main_ir.ir.contains("store i32 40"));
    assert!(main_ir.ir.contains("store i32 2"));
    assert!(main_ir.ir.contains("ret i32"));
}

#[test]
fn emits_local_struct_array_field_when_module_has_import() {
    let root = temp_dir("emits_local_struct_array_field_when_module_has_import");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .empty;

struct S {
    x: i32,
}

struct T {
    xs: [256]S,
}

fn main() i32 {
    var t: T = { xs: [{ x: 0 }; 256] };
    t.xs[255].x
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("empty.nia"),
        r#"
pub fn value() i32 {
    0
}
"#,
    )
    .expect("write empty source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("[256 x %nia__m0__d"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_enum_variant_values_and_switch_patterns() {
    let root = temp_dir("emits_imported_enum_variant_values_and_switch_patterns");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .module_enum_defs;

using module_enum_defs::Mode;

fn main() i32 {
    var box = module_enum_defs::make_box();
    switch box.mode {
        module_enum_defs::Mode::A => Mode::A as u8 as i32,
        module_enum_defs::Mode::B => 2,
        _ => 3,
    }
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("module_enum_defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}

pub struct Box {
    mode: Mode,
}

pub fn make_box() Box {
    { mode: Mode::A }
}
"#,
    )
    .expect("write module source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("switch i8"), "{}", main_ir.ir);
}

#[test]
fn emits_using_imported_type_associated_function_call() {
    let root = temp_dir("emits_using_imported_type_associated_function_call");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .module_assoc_defs;

using module_assoc_defs::Box;

fn main() i32 {
    var box = Box::make(42);
    box.value
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("module_assoc_defs.nia"),
        r#"
pub struct Box {
    value: i32,
}

extend Box {
    pub fn make(value: i32) Box {
        { value: value }
    }
}
"#,
    )
    .expect("write module source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(
        main_ir.ir.contains("call void @nia__m1__"),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_size_builtin_when_module_has_import() {
    let root = temp_dir("emits_size_builtin_when_module_has_import");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .empty;

struct S {
    x: i32,
}

fn main() i32 {
    @size[S]() as i32
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("empty.nia"),
        r#"
pub fn value() i32 {
    0
}
"#,
    )
    .expect("write empty source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 4"), "{}", main_ir.ir);
}

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

#[test]
fn rejects_bare_global_as_pointer_initializer() {
    let root = temp_dir("rejects_bare_global_as_pointer_initializer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
var target: i32 = 1;
const p: &i32 = target;

fn main() i32 {
    target
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("global initializer")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn emits_inline_asm_inputs_outputs_and_clobbers() {
    let root = temp_dir("emits_inline_asm_inputs_outputs_and_clobbers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var value: i64 = 0;
    @asm({
        code:
            b\\mov rax, rax
            \\add rax, 0
        ,
        outputs: { rax: value },
        inputs: { rax: 7 },
        clobbers: [b"memory"],
        options: [b"volatile"],
    });
    value as i32
}

"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("asm sideeffect"));
    assert!(ir.contains("mov rax, rax\\0Aadd rax, 0"));
    assert!(ir.contains("={rax},{rax},~{memory}"), "{ir}");
}

#[test]
fn emits_union_storage_and_field_access() {
    let root = temp_dir("emits_union_storage_and_field_access");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
union Bits {
    i: i32,
    f: f32,
}

fn main() i32 {
    var bits: Bits = { i: 42 };
    bits.i
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%nia__m0__d0__Bits"));
    assert!(ir.contains("store i32 42"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_comptime_values_without_runtime_storage() {
    let root = temp_dir("emits_comptime_values_without_runtime_storage");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
comptime answer: i32 = 40 + 2;
const saved: i32 = answer;

fn main() i32 {
    comptime local: i32 = answer;
    local
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 42"), "{ir}");
    assert!(ir.contains("@nia__m0__d"));
    assert!(!ir.contains("answer"), "{ir}");
    assert!(!ir.contains("local"), "{ir}");
}

#[test]
fn emits_arrays_sized_by_imported_comptime_values() {
    let root = temp_dir("emits_arrays_sized_by_imported_comptime_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .config;

fn main() i32 {
    var values: [config::width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("config.nia"),
        r#"
pub comptime width: usize = 4;
"#,
    )
    .expect("write config source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[4 x i32]"));
}

#[test]
fn emits_large_array_repeat_count_from_comptime_binding() {
    let root = temp_dir("emits_large_array_repeat_count_from_comptime_binding");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
comptime N: usize = 16;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[16 x i8]"));
}

#[test]
fn checked_program_smoke_matrix_emits_llvm_ir() {
    for case in emit_smoke_cases() {
        let root = temp_dir(&format!("checked_program_smoke_matrix_{}", case.name));
        write_smoke_case(&root, case);
        let checked =
            nia_driver::check_program(root.join(case.root).to_string_lossy().into_owned());
        assert!(
            checked.diagnostics.is_empty(),
            "{} check diagnostics: {:?}",
            case.name,
            checked.diagnostics
        );
        let output = emit_llvm_ir(&checked.backend_lowering.program);
        assert!(
            output.diagnostics.is_empty(),
            "{} codegen diagnostics: {:?}",
            case.name,
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            checked.backend_lowering.program.modules.len(),
            "{} should emit one LLVM module per backend module",
            case.name
        );
        assert!(
            output
                .modules
                .iter()
                .all(|module| module.ir.contains("source_filename")),
            "{} emitted empty or malformed IR: {:?}",
            case.name,
            output
                .modules
                .iter()
                .map(|module| (&module.name, module.ir.len()))
                .collect::<Vec<_>>()
        );
        let joined_ir = output
            .modules
            .iter()
            .map(|module| module.ir.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined_ir.contains("define"),
            "{} should emit at least one function definition",
            case.name
        );
    }
}

struct EmitSmokeCase {
    name: &'static str,
    root: &'static str,
    files: &'static [(&'static str, &'static str)],
}

fn emit_smoke_cases() -> &'static [EmitSmokeCase] {
    &[
        EmitSmokeCase {
            name: "control_flow_defer_switch",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
extern fn log(x: i32);

enum State: u8 {
    Start,
    Stop,
    _,
}

fn classify(state: State) i32 {
    defer log(1);
    switch state {
        State::Start => return 10,
        State::Stop => return 20,
        _ => return 30,
    }
    0
}

fn main() i32 {
    var total = 0;
    for var i = 0; i < 4; i += 1 {
        defer log(i);
        if i == 1 {
            continue;
        }
        if i == 3 {
            break;
        }
        total += i;
    }
    classify(State::Start) + total
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "generic_cross_module_using_reexports",
            root: "main.nia",
            files: &[
                (
                    "main.nia",
                    r#"
import .facade;

using facade::{Box, make_box, read_box};

fn main() i32 {
    var box: Box[i32] = make_box(40);
    read_box(&const box) + facade::answer
}
"#,
                ),
                (
                    "facade.nia",
                    r#"
import .impl;

pub using impl::{Box, make_box, read_box, answer};
"#,
                ),
                (
                    "impl.nia",
                    r#"
pub comptime answer: i32 = 2;

pub struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    pub fn get(&const self) T {
        self.value
    }
}

pub fn make_box[T](value: T) Box[T] {
    { value: value }
}

pub fn read_box(box: &const Box[i32]) i32 {
    box.get()
}
"#,
                ),
            ],
        },
        EmitSmokeCase {
            name: "static_data_layout_addresses",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
struct Header {
    tag: u8,
    count: i64,
    flag: u8,
}

const header: Header = { tag: 1, count: 2, flag: 3 };
const bytes = c"ok";
const byte_ptr: &const u8 = &const bytes[0];
var global: i32 = 5;
const global_ptr: &i32 = &global;

fn main() i32 {
    global_ptr.* + header.tag as i32 + header.flag as i32 + byte_ptr.* as i32
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "slices_arrays_and_coercions",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
fn sum(xs: &const [i32]) i32 {
    var out = 0;
    for var i: usize = 0; i < @len(xs); i += 1usize {
        out += xs[i];
    }
    out
}

fn fill(xs: &[i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var part = &const xs[1..=2];
    sum(part) + sum([5, 6]) + fill([0, 1])
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "structural_associated_function_pointers",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
type Ptr[T] = &T;

extend[T] Ptr[T] {
    fn null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

fn main(ptr: &i32) i32 {
    var null: &const fn(&i32) bool = &const [&i32]::null;
    var zero: &const fn() usize = &const [&i32]::zero;
    if null(ptr) or [&i32]::null(ptr) {
        zero() as i32
    } else {
        0
    }
}
"#,
            )],
        },
        EmitSmokeCase {
            name: "union_open_enum_and_comptime_lengths",
            root: "main.nia",
            files: &[(
                "main.nia",
                r#"
comptime width: usize = 2 + 2;

union Bits {
    i: i32,
    f: f32,
}

enum Flag: u32 {
    A = 1,
    B = 2,
    _,
}

fn main(flag: Flag) i32 {
    var values: [width]i32 = [10, 20, 30, 40];
    var bits: Bits = { i: values[0] };
    switch flag {
        Flag::A => return bits.i,
        _ => return Flag::B as u32 as i32,
    }
    0
}
"#,
            )],
        },
    ]
}

fn write_smoke_case(root: &std::path::Path, case: &EmitSmokeCase) {
    for (relative, source) in case.files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create smoke case parent directory");
        }
        std::fs::write(path, source).expect("write smoke case source");
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("nia_codegen_llvm_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn assert_substrings_in_order(haystack: &str, needles: &[&str]) {
    let mut offset = 0usize;
    for needle in needles {
        let Some(index) = haystack[offset..].find(needle) else {
            panic!("missing `{needle}` after byte offset {offset}");
        };
        offset += index + needle.len();
    }
}
