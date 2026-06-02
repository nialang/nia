// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

fn single_module_program(
    interner: nia_ty::TyInterner,
    layouts: BackendLayouts,
    structs: Vec<BackendStruct>,
    unions: Vec<BackendUnion>,
    globals: Vec<BackendGlobal>,
    functions: Vec<BackendFunction>,
) -> BackendProgram {
    BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts,
            structs,
            struct_instances: Vec::new(),
            unions,
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals,
            functions,
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    }
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
            optimization: checked.optimization,
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
            optimization: checked.optimization,
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
fn emits_function_body_from_function_ir_when_available() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: Default::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::Integer("2".to_string()),
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 2"), "{ir}");
}

#[test]
fn emits_statement_if_from_function_ir_with_defer_cleanup() {
    let root = temp_dir("emits_statement_if_from_function_ir_with_defer_cleanup");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    if true {
        defer log(1);
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
    assert!(ir.contains("if.then") || ir.contains("fir.bb"), "{ir}");
    assert!(ir.contains("call void @log(i32 1)"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_statement_switch_from_function_ir_with_defer_cleanup() {
    let root = temp_dir("emits_statement_switch_from_function_ir_with_defer_cleanup");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    switch 1 {
        1 => {
            defer log(1);
        },
        _ => {
            defer log(2);
        },
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
    assert!(ir.contains("switch i32"), "{ir}");
    assert!(ir.contains("call void @log(i32 1)"));
    assert!(ir.contains("call void @log(i32 2)"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_statement_loop_from_function_ir_with_defer_cleanup() {
    let root = temp_dir("emits_statement_loop_from_function_ir_with_defer_cleanup");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    loop {
        defer log(1);
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
    assert!(ir.contains("fir.bb"), "{ir}");
    assert!(ir.contains("call void @log(i32 1)"));
    assert!(ir.contains("ret i32 0"));
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
    let body = TypedBody {
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
    };
    let function_body = nia_function_lower::lower_function_body(&body);
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
                function_body: Some(function_body),
                span: Span::default(),
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("field expression references missing field")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_array_length_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let span = Span::default();
    let len_id = GlobalConstExprId {
        module_id: ModuleId(0),
        const_expr_id: ConstExprId(0),
    };
    let elem = interner.primitive(PrimitiveTy::U8);
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(len_id),
        elem,
    });
    let mut comptime = ComptimeCheck::default();
    comptime.array_lengths.insert(len_id, 4);
    let mut program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime,
            layouts: BackendLayouts {
                types: vec![
                    (elem, TypeLayout { size: 1, align: 1 }),
                    (array_ty, TypeLayout { size: 4, align: 1 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![BackendGlobal {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "buffer".to_string(),
                ty: array_ty,
                is_const: false,
                is_extern: true,
                init: None,
                span,
            }],
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };
    program.modules[0].comptime.array_lengths.clear();

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("was not evaluated before LLVM codegen")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_runtime_layout_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let box_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(0),
    };
    let box_ty = interner.intern(TyKind::Nominal {
        def_id: box_id,
        args: Vec::new(),
    });
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: box_id,
                name: "Box".to_string(),
                generics: Vec::new(),
                fields: vec![BackendField {
                    def_id: GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: DefId(1),
                    },
                    name: "value".to_string(),
                    ty: i32_ty,
                    span,
                }],
                is_extern: false,
                span,
            }],
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(2),
                },
                name: "take".to_string(),
                generics: Vec::new(),
                params: vec![BackendParam {
                    local_id: None,
                    name: Some("value".to_string()),
                    receiver: None,
                    ty: box_ty,
                    span,
                }],
                return_type: i32_ty,
                is_extern: true,
                is_variadic: false,
                function_body: None,
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("has no ABI layout")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_function_instance_refs_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let callee_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::FunctionInstance {
                                        def_id: callee_id,
                                        args: vec![i32_ty],
                                    },
                                    args: Vec::new(),
                                },
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("call references missing function instance")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_indexed_function_instances_with_equivalent_type_args() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(10),
    };
    let canonical_struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
    });
    let equivalent_struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
    });
    let span = Span::default();
    let callee_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (canonical_struct_ty, TypeLayout { size: 0, align: 1 }),
                    (equivalent_struct_ty, TypeLayout { size: 0, align: 1 }),
                ],
                structs: vec![(
                    struct_id,
                    StructLayout {
                        layout: TypeLayout { size: 0, align: 1 },
                        fields: Vec::new(),
                    },
                )],
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: struct_id,
                name: "Marker".to_string(),
                generics: Vec::new(),
                fields: Vec::new(),
                is_extern: false,
                span,
            }],
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::FunctionInstance {
                                        def_id: callee_id,
                                        args: vec![equivalent_struct_ty],
                                    },
                                    args: Vec::new(),
                                },
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: vec![BackendFunctionInstance {
                def_id: callee_id,
                name: "make".to_string(),
                arg_module_id: ModuleId(0),
                args: vec![canonical_struct_ty],
                symbol: "make_marker".to_string(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("@make_marker()"));
}

#[test]
fn validates_backend_ir_missing_vtable_function_refs_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let object_ty = interner.intern(TyKind::TraitObject {
        is_const: true,
        trait_id: TraitId::Source(GlobalDefId {
            module_id: ModuleId(0),
            def_id: DefId(0),
        }),
        trait_args: Vec::new(),
        associated_type_bindings: Vec::new(),
    });
    let span = Span::default();
    let missing_fn = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: vec![BackendTraitObjectVtable {
                key: BackendTraitObjectVtableKey {
                    self_ty: i32_ty,
                    object_ty,
                },
                trait_id: TraitId::Source(GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                }),
                trait_args: Vec::new(),
                entries: vec![BackendTraitObjectVtableEntry {
                    trait_id: TraitId::Source(GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: DefId(0),
                    }),
                    method_id: missing_fn,
                    method_name: "show".to_string(),
                    slot: 0,
                    function: BackendTraitObjectVtableFunction::Function(missing_fn),
                }],
                span,
            }],
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("vtable references missing function")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_initializer_refs_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let ptr_ty = interner.intern(TyKind::Pointer {
        is_const: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let missing_global = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(9),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (ptr_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![BackendGlobal {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "ptr".to_string(),
                ty: ptr_ty,
                is_const: true,
                is_extern: false,
                init: Some(StaticInit::AddrOfGlobal {
                    global: missing_global,
                    path: Vec::new(),
                }),
                span,
            }],
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("static initializer references missing global")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_initializer_field_refs_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(0),
    };
    let field_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let missing_field = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(2),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
    });
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (struct_ty, TypeLayout { size: 4, align: 4 }),
                ],
                structs: vec![(
                    struct_id,
                    StructLayout {
                        layout: TypeLayout { size: 4, align: 4 },
                        fields: vec![FieldLayout {
                            def_id: field_id.def_id,
                            offset: 0,
                            layout: TypeLayout { size: 4, align: 4 },
                        }],
                    },
                )],
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: struct_id,
                name: "Point".to_string(),
                generics: Vec::new(),
                fields: vec![BackendField {
                    def_id: field_id,
                    name: "x".to_string(),
                    ty: i32_ty,
                    span,
                }],
                is_extern: false,
                span,
            }],
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![BackendGlobal {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(3),
                },
                name: "point".to_string(),
                ty: struct_ty,
                is_const: true,
                is_extern: false,
                init: Some(StaticInit::Struct(vec![StaticFieldInit {
                    field: Some(missing_field),
                    value: StaticInit::Int(1),
                }])),
                span,
            }],
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("static initializer references missing field")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_enum_variant_refs_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let missing_variant = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(3),
    };
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: vec![BackendEnum {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "Mode".to_string(),
                backing_type: i32_ty,
                variants: vec![BackendEnumVariant {
                    def_id: GlobalDefId {
                        module_id: ModuleId(0),
                        def_id: DefId(1),
                    },
                    name: "Known".to_string(),
                    value: Some(0),
                    span,
                }],
                span,
            }],
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(2),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::EnumVariant(missing_variant),
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("expression references missing enum variant")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_ir_missing_entry_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: Vec::new(),
                    entry: FunctionBlockId(99),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("backend IR contains invalid function IR")),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function entry block")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_ir_missing_successor_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: ModuleId(0),
            name: "main".to_string(),
            interner,
            comptime: ComptimeCheck::default(),
            layouts: BackendLayouts {
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(0),
                },
                name: "main".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                function_body: Some(FunctionBody {
                    span,
                    locals: Vec::new(),
                    scopes: vec![FunctionScope {
                        id: FunctionScopeId(0),
                        parent: None,
                        span,
                    }],
                    blocks: vec![FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Branch {
                            target: FunctionBlockId(1),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: i32_ty,
                }),
                span,
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }],
    };

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("terminator successor references missing block")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_function_address_refs_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let fn_ptr_ty = interner.intern(TyKind::FunctionPointer {
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
    });
    let span = Span::default();
    let missing_function = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(9),
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (fn_ptr_ty, TypeLayout { size: 8, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![BackendGlobal {
            def_id: GlobalDefId {
                module_id: ModuleId(0),
                def_id: DefId(0),
            },
            name: "ptr".to_string(),
            ty: fn_ptr_ty,
            is_const: true,
            is_extern: false,
            init: Some(StaticInit::AddrOfFunction {
                function: missing_function,
                args: Vec::new(),
            }),
            span,
        }],
        Vec::new(),
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("static initializer references missing function")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_address_path_shape_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let ptr_ty = interner.intern(TyKind::Pointer {
        is_const: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let source_global = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(0),
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (ptr_ty, TypeLayout { size: 8, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![
            BackendGlobal {
                def_id: source_global,
                name: "value".to_string(),
                ty: i32_ty,
                is_const: false,
                is_extern: false,
                init: Some(StaticInit::Int(0)),
                span,
            },
            BackendGlobal {
                def_id: GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: DefId(1),
                },
                name: "ptr".to_string(),
                ty: ptr_ty,
                is_const: true,
                is_extern: false,
                init: Some(StaticInit::AddrOfGlobal {
                    global: source_global,
                    path: vec![nia_static_ir::StaticAddressElem::Index(0)],
                }),
                span,
            },
        ],
        Vec::new(),
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("indexes non-array type")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_aggregate_literal_field_before_llvm() {
    let mut interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(0),
    };
    let field_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(1),
    };
    let missing_field = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(9),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
    });
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: ModuleId(0),
            def_id: DefId(2),
        },
        name: "main".to_string(),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: struct_ty,
        is_extern: false,
        is_variadic: false,
        function_body: Some(FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty: struct_ty,
                        kind: FunctionExprKind::StructLiteral {
                            def_id: struct_id,
                            fields: vec![FunctionFieldInit {
                                field: Some(missing_field),
                                name: "missing".to_string(),
                                value: FunctionExpr {
                                    span,
                                    ty: i32_ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                },
                                span,
                            }],
                        },
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: struct_ty,
        }),
        span,
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (struct_ty, TypeLayout { size: 4, align: 4 }),
            ],
            structs: vec![(
                struct_id,
                StructLayout {
                    layout: TypeLayout { size: 4, align: 4 },
                    fields: vec![FieldLayout {
                        def_id: field_id.def_id,
                        offset: 0,
                        layout: TypeLayout { size: 4, align: 4 },
                    }],
                },
            )],
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        vec![BackendStruct {
            def_id: struct_id,
            name: "Box".to_string(),
            generics: Vec::new(),
            fields: vec![BackendField {
                def_id: field_id,
                name: "value".to_string(),
                ty: i32_ty,
                span,
            }],
            is_extern: false,
            span,
        }],
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("aggregate literal references missing field")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_local_place_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: ModuleId(0),
            def_id: DefId(0),
        },
        name: "main".to_string(),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        function_body: Some(FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: vec![FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: i32_ty,
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span,
                        ty: i32_ty,
                        base: FunctionPlaceBase::Local(LocalId(99)),
                        elems: Vec::new(),
                    }),
                })],
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Integer("0".to_string()),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("place local references missing local")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_unresolved_trait_method_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let trait_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(10),
    };
    let method_id = GlobalDefId {
        module_id: ModuleId(0),
        def_id: DefId(11),
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: ModuleId(0),
            def_id: DefId(0),
        },
        name: "main".to_string(),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        function_body: Some(FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::TraitMethod {
                                trait_id,
                                method_id,
                                method_name: "value".to_string(),
                                self_ty: i32_ty,
                                trait_args: Vec::new(),
                                args: Vec::new(),
                                receiver: Box::new(FunctionExpr {
                                    span,
                                    ty: i32_ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                }),
                            },
                            args: Vec::new(),
                        },
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unresolved trait method")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_unresolved_builtin_place_method_before_llvm() {
    let interner = nia_ty::TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: ModuleId(0),
            def_id: DefId(0),
        },
        name: "main".to_string(),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        function_body: Some(FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::BuiltinPlaceMethod {
                                trait_id: BuiltinTrait::Slice,
                                method: BuiltinTraitMethod::Slice,
                                self_ty: i32_ty,
                                trait_args: Vec::new(),
                                receiver: Box::new(FunctionExpr {
                                    span,
                                    ty: i32_ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                }),
                            },
                            args: Vec::new(),
                        },
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        interner,
        BackendLayouts {
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    let output = emit_llvm_ir(&program);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("unresolved builtin place method")),
        "{:?}",
        output.diagnostics
    );
}
