// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendLayouts, BackendModule, BackendProgram, BackendStruct,
    TypedBody, TypedExpr, TypedExprKind, TypedLocal, TypedLocalKind,
};
use nia_ids::{DefId, GlobalDefId, LocalId, ModuleId};
use nia_layout::{FieldLayout, StructLayout, TypeLayout};
use nia_ty::{PrimitiveTy, TyKind};

#[test]
fn emits_declarations_for_checked_program() {
    let root = temp_dir("emits_declarations_for_checked_program");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &u8) i32;
const hello = "hello\0";

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
    let mut interner = nia_ty::TyInterner::new();
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
    first(borrow) + first(literal) + first([7, 8]) + first_byte("hi\0") + overwrite([6, 7])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("slicetmp"), "{ir}");
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
const hello = "hello\0";

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
    var ch = 'A';
    var byte = b'A';
    _ = ch;
    text[0] as i32 + byte as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("[2 x i8] c\"A\\00\"") || ir.contains("[2 x i8] [i8 65, i8 0]"));
    assert!(ir.contains("store i32 65"));
    assert!(ir.contains("store i8 65"));
    assert!(ir.contains("ret i32"));
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
    assert!(ir.contains("call void @log(i32 11)"));
    assert!(ir.contains("call void @log(i32 10)"));
    assert!(ir.contains("call void @log(i32 12)") || ir.contains("call void @log(i32 12,"));
    assert!(ir.contains("ret i32 1"));
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
    assert!(ir.contains("call void @log(i32 21)"));
    assert!(ir.contains("call void @log(i32 20)"));
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
            \\mov rax, rax
            \\add rax, 0
        ,
        outputs: { rax: value },
        inputs: { rax: 7 },
        clobbers: ["memory"],
        options: ["volatile"],
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

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("nia_codegen_llvm_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
