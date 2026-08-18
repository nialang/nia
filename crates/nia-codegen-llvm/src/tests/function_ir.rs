// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use nia_ast::{BinaryOp, UnaryOp};
use nia_backend_ir::BackendEnumVariantPayload;
use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionArrayElements, FunctionAtomic, FunctionBitIntrinsicOp,
    FunctionMemoryIntrinsic, FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource,
    FunctionSliceRange,
};

fn single_module_program(
    module_id: ModuleId,
    layouts: BackendLayouts,
    structs: Vec<BackendStruct>,
    unions: Vec<BackendUnion>,
    globals: Vec<BackendGlobal>,
    functions: Vec<BackendFunction>,
) -> BackendProgram {
    BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts,
            structs,
            struct_instances: Vec::new(),
            unions,
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals,
            global_instances: Vec::new(),
            functions,
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    }
}

#[test]
fn emits_function_body_from_function_ir_when_available() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: Default::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 2"), "{ir}");
}

#[test]
fn validates_function_return_runtime_layout_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let opaque_ty = interner.intern(TyKind::Opaque);
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("opaque_return"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: opaque_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span: Span::default(),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "return type"
    ));
    assert!(output.modules.is_empty());
}

#[test]
fn validates_static_array_initializer_length_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(2),
        elem: u8_ty,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("oversized"),
        link_name: None,
        ty: array_ty,
        is_let: false,
        is_extern: false,
        init: Some(StaticInit::Repeat {
            value: Box::new(StaticInit::Byte(0)),
            count: u64::MAX,
        }),
        span: Span::default(),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (u8_ty, TypeLayout { size: 1, align: 1 }),
                (array_ty, TypeLayout { size: 2, align: 1 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![global],
        Vec::new(),
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "repeat static initializer has"
    ));
}

#[test]
fn validates_layout_builtin_array_length_for_32_bit_target() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let pointer_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: u8_ty,
    });
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::Builtin {
            builtin: nia_ty::LayoutBuiltin::Size,
            ty: pointer_ty,
        },
        elem: u8_ty,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("pointer_bytes"),
        link_name: None,
        ty: array_ty,
        is_let: false,
        is_extern: false,
        init: Some(StaticInit::Repeat {
            value: Box::new(StaticInit::Byte(0)),
            count: 4,
        }),
        span: Span::default(),
    };
    let target = nia_layout::TargetDataLayout {
        pointer_size: 4,
        pointer_align: 4,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target,
            types: vec![
                (u8_ty, TypeLayout { size: 1, align: 1 }),
                (array_ty, TypeLayout { size: 4, align: 1 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![global],
        Vec::new(),
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[4 x i8]"));
}

#[test]
fn emits_pointer_sized_integer_abi_for_32_bit_target() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("word"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: usize_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                        ty: usize_ty,
                        kind: FunctionExprKind::BuiltinValue(
                            nia_function_ir::FunctionBuiltinValue::Usize(7),
                        ),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: usize_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout {
                pointer_size: 4,
                pointer_align: 4,
            },
            // Exercise the target-aware validator/codegen fallback rather than
            // satisfying it from a precomputed per-type layout.
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32"), "{ir}");
    assert!(ir.contains("ret i32 7"), "{ir}");
}

#[test]
fn emits_bitmask_with_32_bit_usize_result() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let boolx16_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::Bool,
        lanes: 16,
    });
    let span = Span::default();
    let vector = FunctionExpr {
        span,
        ty: boolx16_ty,
        kind: FunctionExprKind::Splat {
            value: Box::new(FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::Bool(true),
            }),
        },
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("mask"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: usize_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                        ty: usize_ty,
                        kind: FunctionExprKind::Bitmask {
                            vector: Box::new(vector),
                        },
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: usize_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout {
                pointer_size: 4,
                pointer_align: 4,
            },
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32"), "{ir}");
    assert!(ir.contains("ret i32 65535"), "{ir}");
    assert!(!ir.contains("ret i64"), "{ir}");
}

#[test]
fn validates_terminator_type_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let span = Span::default();
    let int_expr = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Integer("1".to_string()),
    };
    let layouts = BackendLayouts {
        target: nia_layout::TargetDataLayout::LP64,
        types: vec![
            (i32_ty, TypeLayout { size: 4, align: 4 }),
            (bool_ty, TypeLayout { size: 1, align: 1 }),
        ],
        structs: Vec::new(),
        unions: Vec::new(),
        enums: Vec::new(),
        struct_instances: Vec::new(),
        union_instances: Vec::new(),
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![
                FunctionBlock {
                    id: FunctionBlockId(0),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::If {
                        cond: int_expr(i32_ty),
                        then_target: FunctionBlockId(1),
                        else_target: FunctionBlockId(2),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(1),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Tail {
                        value: Some(int_expr(i32_ty)),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(2),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Tail {
                        value: Some(FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        }),
                        span,
                    },
                },
            ],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    drop(interner);

    let output = emit_owned_llvm_ir(
        single_module_program(
            module_id,
            layouts,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![function],
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("control-flow condition must have type bool")),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("return value type does not match")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_projection_and_field_initializer_types_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let tuple_ty = interner.intern(TyKind::Tuple(vec![i32_ty]));
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: i32_ty,
    });
    let slice_i32_ty = interner.intern(TyKind::Slice {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_slice_i32_ty = interner.intern(TyKind::Slice {
        is_readonly: true,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let field_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let integer = || FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Integer("1".to_string()),
    };
    let body = |ty, value| FunctionBody {
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
                value: Some(value),
                span,
            },
        }],
        entry: FunctionBlockId(0),
        ty,
    };
    let function = |def_id, name, return_type, value| BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        },
        name: sym(name),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(body(return_type, value)),
        span,
    };
    let functions = vec![
        function(
            2,
            "bad_init",
            struct_ty,
            FunctionExpr {
                span,
                ty: struct_ty,
                kind: FunctionExprKind::StructLiteral {
                    def_id: struct_id,
                    fields: vec![FunctionFieldInit {
                        field: Some(field_id),
                        name: "value".to_string(),
                        value: FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                        span,
                    }],
                },
            },
        ),
        function(
            4,
            "bad_tuple",
            bool_ty,
            FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::TupleField {
                    value: Box::new(FunctionExpr {
                        span,
                        ty: tuple_ty,
                        kind: FunctionExprKind::Tuple(vec![integer()]),
                    }),
                    index: 0,
                },
            },
        ),
        function(
            9,
            "bad_field",
            bool_ty,
            FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::Field {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: struct_ty,
                        kind: FunctionExprKind::Null,
                    }),
                    field: field_id,
                },
            },
        ),
        function(
            10,
            "bad_slice_source",
            bool_ty,
            FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::Slice {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: bool_ty,
                        kind: FunctionExprKind::Bool(true),
                    }),
                    range: FunctionSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: false,
                },
            },
        ),
        function(
            11,
            "bad_slice_element",
            interner.intern(TyKind::Slice {
                is_readonly: false,
                elem: bool_ty,
            }),
            FunctionExpr {
                span,
                ty: interner.intern(TyKind::Slice {
                    is_readonly: false,
                    elem: bool_ty,
                }),
                kind: FunctionExprKind::Slice {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: array_ty,
                        kind: FunctionExprKind::ArrayLiteral {
                            elems: FunctionArrayElements::List(vec![integer()]),
                        },
                    }),
                    range: FunctionSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: false,
                },
            },
        ),
        function(
            12,
            "bad_slice_readonly",
            slice_i32_ty,
            FunctionExpr {
                span,
                ty: slice_i32_ty,
                kind: FunctionExprKind::Slice {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: readonly_i32_ptr_ty,
                        kind: FunctionExprKind::Null,
                    }),
                    range: FunctionSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: false,
                },
            },
        ),
        function(
            13,
            "bad_slice_bound",
            readonly_slice_i32_ty,
            FunctionExpr {
                span,
                ty: readonly_slice_i32_ty,
                kind: FunctionExprKind::Slice {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: array_ty,
                        kind: FunctionExprKind::ArrayLiteral {
                            elems: FunctionArrayElements::List(vec![integer()]),
                        },
                    }),
                    range: FunctionSliceRange {
                        start: Some(Box::new(FunctionExpr {
                            span,
                            ty: f32_ty,
                            kind: FunctionExprKind::Float("1.0".to_string()),
                        })),
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: true,
                },
            },
        ),
        function(
            5,
            "bad_index",
            bool_ty,
            FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::Index {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: array_ty,
                        kind: FunctionExprKind::ArrayLiteral {
                            elems: FunctionArrayElements::List(vec![integer()]),
                        },
                    }),
                    index: Box::new(FunctionExpr {
                        span,
                        ty: usize_ty,
                        kind: FunctionExprKind::BuiltinValue(
                            nia_function_ir::FunctionBuiltinValue::Usize(0),
                        ),
                    }),
                },
            },
        ),
        function(
            6,
            "bad_tuple_literal",
            tuple_ty,
            FunctionExpr {
                span,
                ty: tuple_ty,
                kind: FunctionExprKind::Tuple(vec![FunctionExpr {
                    span,
                    ty: bool_ty,
                    kind: FunctionExprKind::Bool(true),
                }]),
            },
        ),
        function(
            7,
            "bad_array_literal",
            array_ty,
            FunctionExpr {
                span,
                ty: array_ty,
                kind: FunctionExprKind::ArrayLiteral {
                    elems: FunctionArrayElements::List(vec![
                        FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                        integer(),
                    ]),
                },
            },
        ),
        function(
            8,
            "bad_array_repeat",
            array_ty,
            FunctionExpr {
                span,
                ty: array_ty,
                kind: FunctionExprKind::ArrayLiteral {
                    elems: FunctionArrayElements::Repeat {
                        value: Box::new(integer()),
                        count: ArrayLenTy::ConstValue(2),
                    },
                },
            },
        ),
    ];
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (bool_ty, TypeLayout { size: 1, align: 1 }),
                (f32_ty, TypeLayout { size: 4, align: 4 }),
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (usize_ty, TypeLayout { size: 8, align: 8 }),
                (tuple_ty, TypeLayout { size: 4, align: 4 }),
                (array_ty, TypeLayout { size: 4, align: 4 }),
                (struct_ty, TypeLayout { size: 4, align: 4 }),
                (slice_i32_ty, TypeLayout { size: 16, align: 8 }),
                (readonly_slice_i32_ty, TypeLayout { size: 16, align: 8 }),
                (readonly_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
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
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        vec![BackendStruct {
            def_id: struct_id,
            name: sym("Box"),
            generics: Vec::new(),
            fields: vec![BackendField {
                def_id: field_id,
                name: sym("value"),
                ty: i32_ty,
                span,
            }],
            is_extern: false,
            span,
        }],
        Vec::new(),
        Vec::new(),
        functions,
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "aggregate field initializer result type does not match",
        "field result type does not match",
        "tuple result type does not match",
        "index result type does not match",
        "slice input is not an array, pointer, or slice",
        "slice result element does not match its input",
        "slice drops readonly access from its input",
        "slice range bound is not an integer",
        "tuple literal has an invalid type contract: element type",
        "array literal has an invalid type contract: element count",
        "array literal has an invalid type contract: element type",
        "array repeat literal has an invalid type contract: count",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_atomic_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let unit_ty = interner.intern(TyKind::Tuple(Vec::new()));
    let optional_i32_ty = interner.intern(TyKind::Optional { elem: i32_ty });
    let readonly_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let mutable_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let mutable_bool_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_ty,
    });
    let mutable_ptr_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: mutable_i32_ptr_ty,
    });
    let readonly_f32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: f32_ty,
    });
    let span = Span::default();
    let null = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let integer = || FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Integer("1".to_string()),
    };
    let boolean = || FunctionExpr {
        span,
        ty: bool_ty,
        kind: FunctionExprKind::Bool(true),
    };
    let atomic = |ty, atomic| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Atomic(atomic),
    };
    let ops = vec![
        FunctionOp::Expr(atomic(
            bool_ty,
            FunctionAtomic::Load {
                ty: i32_ty,
                ptr: Box::new(null(mutable_bool_ptr_ty)),
                order: AtomicOrder::Release,
            },
        )),
        FunctionOp::Expr(atomic(
            optional_i32_ty,
            FunctionAtomic::Cmpxchg {
                ty: i32_ty,
                ptr: Box::new(null(mutable_i32_ptr_ty)),
                expected: Box::new(integer()),
                desired: Box::new(integer()),
                success: AtomicOrder::Release,
                failure: AtomicOrder::Acquire,
                weak: true,
            },
        )),
        FunctionOp::Expr(atomic(
            unit_ty,
            FunctionAtomic::Store {
                ty: i32_ty,
                ptr: Box::new(null(readonly_i32_ptr_ty)),
                value: Box::new(boolean()),
                order: AtomicOrder::Acquire,
            },
        )),
        FunctionOp::Expr(atomic(
            i32_ty,
            FunctionAtomic::Rmw {
                ty: i32_ty,
                ptr: Box::new(null(mutable_i32_ptr_ty)),
                op: AtomicRmwOp::Add,
                value: Box::new(integer()),
                order: AtomicOrder::Unordered,
            },
        )),
        FunctionOp::Expr(atomic(
            mutable_i32_ptr_ty,
            FunctionAtomic::Rmw {
                ty: mutable_i32_ptr_ty,
                ptr: Box::new(null(mutable_ptr_ptr_ty)),
                op: AtomicRmwOp::Add,
                value: Box::new(null(mutable_i32_ptr_ty)),
                order: AtomicOrder::Monotonic,
            },
        )),
        FunctionOp::Expr(atomic(
            bool_ty,
            FunctionAtomic::Cmpxchg {
                ty: i32_ty,
                ptr: Box::new(null(mutable_i32_ptr_ty)),
                expected: Box::new(boolean()),
                desired: Box::new(integer()),
                success: AtomicOrder::Monotonic,
                failure: AtomicOrder::SeqCst,
                weak: false,
            },
        )),
        FunctionOp::Expr(atomic(
            unit_ty,
            FunctionAtomic::Fence {
                order: AtomicOrder::Monotonic,
            },
        )),
        FunctionOp::Expr(atomic(
            f32_ty,
            FunctionAtomic::Load {
                ty: f32_ty,
                ptr: Box::new(null(readonly_f32_ptr_ty)),
                order: AtomicOrder::Acquire,
            },
        )),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_atomics"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(integer()),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (bool_ty, TypeLayout { size: 1, align: 1 }),
                (f32_ty, TypeLayout { size: 4, align: 4 }),
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (unit_ty, TypeLayout { size: 0, align: 1 }),
                (optional_i32_ty, TypeLayout { size: 8, align: 4 }),
                (readonly_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (mutable_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (mutable_bool_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (mutable_ptr_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (readonly_f32_ptr_ty, TypeLayout { size: 8, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "pointer pointee does not match",
        "load type does not match",
        "mutating operation has a readonly pointer",
        "store value type does not match",
        "cmpxchg expected value type does not match",
        "cmpxchg result must be optional",
        "cmpxchg failure ordering is stronger than or incomparable",
        "non-exchange RMW operation requires an integer-like",
        "ordering is invalid for the operation",
        "value type is not a pointer-width",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("cmpxchg failure ordering is stronger than or incomparable"))
            .count(),
        2,
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_memory_intrinsic_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let mutable_i32_slice_ty = interner.intern(TyKind::Slice {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_slice_ty = interner.intern(TyKind::Slice {
        is_readonly: true,
        elem: i32_ty,
    });
    let readonly_bool_slice_ty = interner.intern(TyKind::Slice {
        is_readonly: true,
        elem: bool_ty,
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let memory = |op, elem_ty, dest_ty, source| {
        FunctionOp::MemoryIntrinsic(Box::new(FunctionMemoryIntrinsic {
            span,
            op,
            elem_ty,
            dest: value(dest_ty),
            source,
        }))
    };
    let ops = vec![
        memory(
            FunctionMemoryIntrinsicOp::Copy,
            i32_ty,
            readonly_i32_slice_ty,
            FunctionMemoryIntrinsicSource::Slice(value(readonly_bool_slice_ty)),
        ),
        memory(
            FunctionMemoryIntrinsicOp::Move,
            i32_ty,
            i32_ty,
            FunctionMemoryIntrinsicSource::Slice(value(readonly_i32_slice_ty)),
        ),
        memory(
            FunctionMemoryIntrinsicOp::Copy,
            i32_ty,
            mutable_i32_slice_ty,
            FunctionMemoryIntrinsicSource::Byte(value(u8_ty)),
        ),
        memory(
            FunctionMemoryIntrinsicOp::Set,
            i32_ty,
            mutable_i32_slice_ty,
            FunctionMemoryIntrinsicSource::Byte(value(bool_ty)),
        ),
        memory(
            FunctionMemoryIntrinsicOp::Set,
            u8_ty,
            mutable_i32_slice_ty,
            FunctionMemoryIntrinsicSource::Slice(value(readonly_i32_slice_ty)),
        ),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_memory_intrinsics"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(value(i32_ty)),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (bool_ty, TypeLayout { size: 1, align: 1 }),
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (u8_ty, TypeLayout { size: 1, align: 1 }),
                (mutable_i32_slice_ty, TypeLayout { size: 16, align: 8 }),
                (readonly_i32_slice_ty, TypeLayout { size: 16, align: 8 }),
                (readonly_bool_slice_ty, TypeLayout { size: 16, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "destination slice is readonly",
        "destination is not a slice",
        "destination element type does not match",
        "source element type does not match",
        "copy or move operation requires a slice source",
        "set operation element type is not u8",
        "set source is not a u8 value",
        "set operation requires a byte source",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_low_level_builtin_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let u32_ty = interner.primitive(PrimitiveTy::U32);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let optional_char_ty = interner.intern(TyKind::Optional { elem: char_ty });
    let byte_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: u8_ty,
    });
    let i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let i32x4_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let boolx4_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::Bool,
        lanes: 4,
    });
    let boolx65_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::Bool,
        lanes: 65,
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let builtin = |ty, kind| FunctionOp::Expr(FunctionExpr { span, ty, kind });
    let ops = vec![
        builtin(
            bool_ty,
            FunctionExprKind::LoadUnaligned {
                ty: i32_ty,
                ptr: Box::new(value(i32_ptr_ty)),
            },
        ),
        builtin(
            i32x4_ty,
            FunctionExprKind::Splat {
                value: Box::new(value(bool_ty)),
            },
        ),
        builtin(
            i32_ty,
            FunctionExprKind::Splat {
                value: Box::new(value(i32_ty)),
            },
        ),
        builtin(
            bool_ty,
            FunctionExprKind::ExtractElement {
                vector: Box::new(value(i32x4_ty)),
                index: Box::new(value(f32_ty)),
            },
        ),
        builtin(
            boolx4_ty,
            FunctionExprKind::InsertElement {
                vector: Box::new(value(i32x4_ty)),
                index: Box::new(value(f32_ty)),
                value: Box::new(value(bool_ty)),
            },
        ),
        builtin(
            i32_ty,
            FunctionExprKind::Bitmask {
                vector: Box::new(value(i32x4_ty)),
            },
        ),
        builtin(
            usize_ty,
            FunctionExprKind::Bitmask {
                vector: Box::new(value(boolx65_ty)),
            },
        ),
        builtin(
            u32_ty,
            FunctionExprKind::BitIntrinsic {
                op: FunctionBitIntrinsicOp::Ctz,
                value: Box::new(value(i32_ty)),
            },
        ),
        builtin(
            f32_ty,
            FunctionExprKind::BitIntrinsic {
                op: FunctionBitIntrinsicOp::Popcount,
                value: Box::new(value(f32_ty)),
            },
        ),
        builtin(
            optional_char_ty,
            FunctionExprKind::CharFromU32 {
                value: Box::new(value(i32_ty)),
            },
        ),
        builtin(
            i32_ty,
            FunctionExprKind::CharFromU32 {
                value: Box::new(value(u32_ty)),
            },
        ),
        builtin(
            i32_ty,
            FunctionExprKind::LoadUnaligned {
                ty: i32_ty,
                ptr: Box::new(value(byte_ptr_ty)),
            },
        ),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_low_level_builtins"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(value(i32_ty)),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "unaligned load has an invalid contract: result type",
        "unaligned load has an invalid contract: operand is not a byte pointer",
        "SIMD splat has an invalid contract: scalar value type",
        "SIMD splat has an invalid contract: result is not a vector",
        "SIMD lane has an invalid contract: index",
        "SIMD extract has an invalid contract: result type",
        "SIMD insert has an invalid contract: result type",
        "SIMD insert has an invalid contract: inserted value type",
        "SIMD bitmask has an invalid contract: result type",
        "SIMD bitmask has an invalid contract: operand is not a bool vector",
        "SIMD bitmask has an invalid contract: mask exceeds the target usize width",
        "bit intrinsic has an invalid contract: operand",
        "bit intrinsic has an invalid contract: result type",
        "char conversion has an invalid contract: operand type",
        "char conversion has an invalid contract: result type",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_unary_and_binary_operator_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let vector_i32_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let pointer_i32_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let unary = |ty, op, inner| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Unary {
                op,
                expr: Box::new(value(inner)),
            },
        })
    };
    let binary = |ty, lhs, op, rhs| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(value(lhs)),
                op,
                rhs: Box::new(value(rhs)),
            },
        })
    };
    let ops = vec![
        unary(bool_ty, UnaryOp::Neg, bool_ty),
        unary(bool_ty, UnaryOp::Deref, i32_ty),
        unary(i32_ty, UnaryOp::BitNot, f32_ty),
        binary(i32_ty, bool_ty, BinaryOp::Add, i32_ty),
        binary(bool_ty, i32_ty, BinaryOp::Add, i32_ty),
        binary(i32_ty, i32_ty, BinaryOp::Eq, f32_ty),
        binary(bool_ty, i32_ty, BinaryOp::And, bool_ty),
        binary(i32_ty, i32_ty, BinaryOp::Shl, f32_ty),
        unary(i32_ty, UnaryOp::Deref, pointer_i32_ty),
        binary(vector_i32_ty, vector_i32_ty, BinaryOp::Add, vector_i32_ty),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_operators"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(value(i32_ty)),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "negation operand is not numeric",
        "deref operand is not a pointer",
        "bitwise unary operand is not integer-like",
        "binary operands do not have a compatible type",
        "binary operand type is not supported",
        "binary result type does not match",
        "logical operator requires bool operands",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_cast_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let i64_ty = interner.primitive(PrimitiveTy::I64);
    let u32_ty = interner.primitive(PrimitiveTy::U32);
    let vector_i32x4_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let vector_i32x8_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 8,
    });
    let vector_i64x4_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I64,
        lanes: 4,
    });
    let vector_i64x8_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I64,
        lanes: 8,
    });
    let pointer_i32_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let volatile_pointer_i32_ty = interner.intern(TyKind::VolatilePointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let cast = |result_ty, target_ty, source_ty| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: result_ty,
            kind: FunctionExprKind::Cast {
                expr: Box::new(value(source_ty)),
                ty: target_ty,
            },
        })
    };
    let ops = vec![
        cast(bool_ty, i64_ty, i32_ty),
        cast(i64_ty, i64_ty, f32_ty),
        cast(i64_ty, i64_ty, pointer_i32_ty),
        cast(pointer_i32_ty, pointer_i32_ty, i32_ty),
        cast(vector_i64x8_ty, vector_i64x8_ty, vector_i32x4_ty),
        cast(vector_i64x4_ty, vector_i64x4_ty, i32_ty),
        cast(i32_ty, u32_ty, char_ty),
        cast(pointer_i32_ty, pointer_i32_ty, volatile_pointer_i32_ty),
        cast(vector_i64x8_ty, vector_i64x8_ty, vector_i32x8_ty),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_casts"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(value(i32_ty)),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "cast result type does not match its target metadata",
        "cast source and target categories are incompatible",
        "numeric cast changes scalar/vector shape",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_tagged_union_expression_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let optional_i32_ty = interner.intern(TyKind::Optional { elem: i32_ty });
    let error_union_ty = interner.intern(TyKind::ErrorUnion {
        error: bool_ty,
        value: i32_ty,
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let tagged = |result_ty, kind| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: result_ty,
            kind,
        })
    };
    let ops = vec![
        tagged(
            i32_ty,
            FunctionExprKind::OptionalSome {
                expr: Box::new(value(bool_ty)),
            },
        ),
        tagged(
            optional_i32_ty,
            FunctionExprKind::ErrorOk {
                expr: Box::new(value(bool_ty)),
            },
        ),
        tagged(
            error_union_ty,
            FunctionExprKind::ErrorErr {
                expr: Box::new(value(i32_ty)),
            },
        ),
        tagged(
            bool_ty,
            FunctionExprKind::TaggedUnionTag {
                expr: Box::new(value(optional_i32_ty)),
            },
        ),
        tagged(
            bool_ty,
            FunctionExprKind::TaggedUnionPayload {
                expr: Box::new(value(optional_i32_ty)),
            },
        ),
        tagged(
            bool_ty,
            FunctionExprKind::TaggedUnionPayload {
                expr: Box::new(value(i32_ty)),
            },
        ),
        tagged(
            u8_ty,
            FunctionExprKind::TaggedUnionTag {
                expr: Box::new(value(i32_ty)),
            },
        ),
        tagged(
            u8_ty,
            FunctionExprKind::TaggedUnionTag {
                expr: Box::new(value(error_union_ty)),
            },
        ),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_tagged_union_exprs"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops,
                terminator: FunctionTerminator::Tail {
                    value: Some(value(i32_ty)),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "constructor result is not the matching Optional or ErrorUnion type",
        "constructor payload type does not match its result",
        "tag projection input is not a tagged union",
        "tag projection result is not u8",
        "optional payload result does not match its element",
        "payload projection input is not a tagged union",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
    match 1 {
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
    let mut default_int = 10;
    let mut default_float = 1.5;
    let mut typed_byte: u8 = 11;
    let mut typed_float: f32 = 3.5;
    let mut byte: u8 = 10;
    let mut signed: i8 = -1;
    let mut float32: f32 = 2.5;
    _ = take_byte(3);
    _ = take32(typed_float);
    default_int + byte as i32 + signed as i32 + typed_byte as i32 + default_float as i32 + float32 as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("store i32 10"));
    assert!(ir.contains("store double"));
    assert!(ir.contains("store i8 11"));
    assert!(ir.contains("store i8 10"));
    assert!(ir.contains("llvm.ssub.with.overflow.i8"));
    assert!(ir.contains("store i8 %arith.value"));
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
    print: &fn(&i32)
}

fn print_i32(value: &i32) {}

static vtable: Vtable = Vtable { print: & print_i32 };

fn main() i32 {
    let mut x = 1;
    vtable.print(&x);
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
    assert_contains_mangled_symbol(ir, '@', "print_i32");
    assert!(ir.contains("call void"), "{ir}");
}

#[test]
fn rejects_field_access_with_mismatched_base_struct() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let point_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let other_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let point_x = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let other_y = GlobalDefId {
        module_id,
        def_id: DefId(3),
    };
    let point_ty = interner.intern(TyKind::Nominal {
        def_id: point_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let body = TypedBody {
        span: Span::default(),
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("point"),
            kind: TypedLocalKind::MutableBinding,
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
    let function_body = nia_function_lower::lower_function_body(
        module_id,
        &body,
        nia_function_lower::FunctionTypeContext::for_module(&type_store, module_id),
    )
    .expect("valid typed body")
    .body;
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
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
                enums: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![
                BackendStruct {
                    def_id: point_id,
                    name: sym("Point"),
                    generics: Vec::new(),
                    fields: vec![BackendField {
                        def_id: point_x,
                        name: sym("x"),
                        ty: i32_ty,
                        span: Span::default(),
                    }],
                    is_extern: false,
                    span: Span::default(),
                },
                BackendStruct {
                    def_id: other_id,
                    name: sym("Other"),
                    generics: Vec::new(),
                    fields: vec![BackendField {
                        def_id: other_y,
                        name: sym("y"),
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
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(4),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: Some(function_body),
                span: Span::default(),
            }],
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "field expression references missing field"
        ),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_array_length_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let span = Span::default();
    let len_id = GlobalConstExprId {
        module_id,
        const_expr_id: ConstExprId(0),
    };
    let elem = interner.primitive(PrimitiveTy::U8);
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(len_id),
        elem,
    });
    let mut const_eval = BackendConstFacts::default();
    const_eval.array_lengths.insert(len_id, 4);
    let mut module = BackendModule {
        id: module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        name: "main".to_string(),
        const_eval,
        layouts: BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (elem, TypeLayout { size: 1, align: 1 }),
                (array_ty, TypeLayout { size: 4, align: 1 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
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
                module_id,
                def_id: DefId(0),
            },
            name: sym("buffer"),
            link_name: None,
            ty: array_ty,
            is_let: false,
            is_extern: true,
            init: None,
            span,
        }],
        global_instances: Vec::new(),
        functions: Vec::new(),
        function_instances: Vec::new(),
        closure_entries: Vec::new(),
        trait_object_vtables: Vec::new(),
        generic_instantiations: Vec::new(),
    };
    module.const_eval.array_lengths.clear();
    let program = BackendProgram::new(vec![module]);

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("was not evaluated before LLVM codegen")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_runtime_layout_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let box_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let box_ty = interner.intern(TyKind::Nominal {
        def_id: box_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: box_id,
                name: sym("Box"),
                generics: Vec::new(),
                fields: vec![BackendField {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(1),
                    },
                    name: sym("value"),
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
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(2),
                },
                name: sym("take"),
                link_name: None,
                generics: Vec::new(),
                params: vec![BackendParam {
                    local_id: None,
                    name: Some(sym("value")),
                    receiver: None,
                    passing_ty: box_ty,
                    local_ty: box_ty,
                    span,
                }],
                return_type: i32_ty,
                is_extern: true,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span,
            }],
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("has no ABI layout")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_error_type_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let error_ty = interner.error();
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("take"),
                link_name: None,
                generics: Vec::new(),
                params: vec![BackendParam {
                    local_id: None,
                    name: Some(sym("value")),
                    receiver: None,
                    passing_ty: error_ty,
                    local_ty: error_ty,
                    span,
                }],
                return_type: i32_ty,
                is_extern: true,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span,
            }],
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("is error")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_propagation_contract_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let optional_i32_ty = interner.intern(TyKind::Optional { elem: i32_ty });
    let source_result_ty = interner.intern(TyKind::ErrorUnion {
        error: i32_ty,
        value: i32_ty,
    });
    let target_result_ty = interner.intern(TyKind::ErrorUnion {
        error: bool_ty,
        value: i32_ty,
    });
    let span = Span::default();
    let function = |def_id: u64,
                    name: &str,
                    input_ty,
                    return_ty,
                    kind,
                    success_ty,
                    error_conversion: Option<FunctionExpr>| {
        BackendFunction {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(def_id),
            },
            name: sym(name),
            link_name: None,
            generics: Vec::new(),
            params: vec![BackendParam {
                local_id: Some(LocalId(0)),
                name: Some(sym("value")),
                receiver: None,
                passing_ty: input_ty,
                local_ty: input_ty,
                span,
            }],
            return_type: return_ty,
            is_extern: false,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: Some(FunctionBody {
                span,
                locals: vec![
                    FunctionLocal {
                        id: LocalId(0),
                        name: local_name("value"),
                        kind: FunctionLocalKind::Param,
                        ty: input_ty,
                        span,
                    },
                    FunctionLocal {
                        id: LocalId(1),
                        name: local_name("success"),
                        kind: FunctionLocalKind::MutableBinding,
                        ty: success_ty,
                        span,
                    },
                ],
                scopes: vec![FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                }],
                blocks: vec![
                    FunctionBlock {
                        id: FunctionBlockId(0),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Try {
                            value: FunctionExpr {
                                span,
                                ty: input_ty,
                                kind: FunctionExprKind::Local(LocalId(0)),
                            },
                            kind,
                            error_conversion: error_conversion.map(Box::new),
                            success_local: LocalId(1),
                            success_target: FunctionBlockId(1),
                            span,
                        },
                    },
                    FunctionBlock {
                        id: FunctionBlockId(1),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail { value: None, span },
                    },
                ],
                entry: FunctionBlockId(0),
                ty: return_ty,
            }),
            span,
        }
    };
    let functions = vec![
        function(
            0,
            "optionalConversion",
            optional_i32_ty,
            optional_i32_ty,
            FunctionTryKind::Optional,
            i32_ty,
            Some(FunctionExpr {
                span,
                ty: bool_ty,
                kind: FunctionExprKind::Bool(false),
            }),
        ),
        function(
            1,
            "kindMismatch",
            optional_i32_ty,
            optional_i32_ty,
            FunctionTryKind::ErrorUnion,
            i32_ty,
            None,
        ),
        function(
            2,
            "successMismatch",
            source_result_ty,
            source_result_ty,
            FunctionTryKind::ErrorUnion,
            bool_ty,
            None,
        ),
        function(
            3,
            "directErrorMismatch",
            source_result_ty,
            target_result_ty,
            FunctionTryKind::ErrorUnion,
            i32_ty,
            None,
        ),
        function(
            4,
            "conversionMismatch",
            source_result_ty,
            target_result_ty,
            FunctionTryKind::ErrorUnion,
            i32_ty,
            Some(FunctionExpr {
                span,
                ty: i32_ty,
                kind: FunctionExprKind::Integer("0".to_string()),
            }),
        ),
    ];
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: Vec::new(),
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        functions,
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);
    let summaries = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();

    assert!(output.modules.is_empty());
    for expected in [
        "optional propagation cannot carry an error conversion",
        "propagation kind does not match its input union type",
        "propagation success local type does not match the input success payload",
        "direct propagation error type does not match the return error payload",
        "propagation conversion type does not match the return error payload",
    ] {
        assert!(
            summaries.iter().any(|summary| summary.contains(expected)),
            "missing `{expected}` in {summaries:?}"
        );
    }
}

#[test]
fn validates_backend_ir_missing_function_instance_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let callee_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
                                        arg_module_id: module_id,
                                        self_arg: None,
                                        args: vec![i32_ty],
                                        const_args: Vec::new(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "call references missing function instance"
        ),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_indexed_function_instances_with_equivalent_type_args() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let canonical_struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let equivalent_struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let callee_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
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
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: struct_id,
                name: sym("Marker"),
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
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
                                        arg_module_id: module_id,
                                        self_arg: None,
                                        args: vec![equivalent_struct_ty],
                                        const_args: Vec::new(),
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
                name: sym("make"),
                arg_module_id: module_id,
                self_arg: None,
                args: vec![canonical_struct_ty],
                const_args: Vec::new(),
                symbol: "make_marker".to_string(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("@make_marker()"));
}

#[test]
fn validates_backend_ir_vtable_structure_and_function_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let object_ty = interner.intern(TyKind::TraitObject {
        is_readonly: true,
        trait_id: TraitId::Source(GlobalDefId {
            module_id,
            def_id: DefId(0),
        }),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: Vec::new(),
    });
    let span = Span::default();
    let missing_fn = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: vec![BackendTraitObjectVtable {
                key: BackendTraitObjectVtableKey {
                    self_ty: i32_ty,
                    object_ty,
                },
                trait_id: TraitId::Source(GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                }),
                trait_args: vec![i32_ty],
                trait_const_args: Vec::new(),
                entries: vec![BackendTraitObjectVtableEntry {
                    trait_id: TraitId::Source(GlobalDefId {
                        module_id,
                        def_id: DefId(0),
                    }),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    method_id: missing_fn,
                    method_name: known::SHOW,
                    slot: 1,
                    function: BackendTraitObjectVtableFunction::Function(missing_fn),
                }],
                span,
            }],
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("vtable trait arguments do not match its object type")),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("vtable entry slot does not match its table position")),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("vtable references missing function")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_dynamic_trait_method_slot_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let bool_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_ty,
    });
    let trait_def = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let method_def = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let secondary_method_def = GlobalDefId {
        module_id,
        def_id: DefId(5),
    };
    let return_method_def = GlobalDefId {
        module_id,
        def_id: DefId(6),
    };
    let child_trait_def = GlobalDefId {
        module_id,
        def_id: DefId(7),
    };
    let object_ty = interner.intern(TyKind::TraitObject {
        is_readonly: true,
        trait_id: TraitId::Source(trait_def),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: Vec::new(),
    });
    let child_object_ty = interner.intern(TyKind::TraitObject {
        is_readonly: true,
        trait_id: TraitId::Source(child_trait_def),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: Vec::new(),
    });
    let span = Span::default();
    let body = |value| FunctionBody {
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
                value: Some(value),
                span,
            },
        }],
        entry: FunctionBlockId(0),
        ty: i32_ty,
    };
    let method = BackendFunction {
        def_id: method_def,
        name: known::SHOW,
        link_name: None,
        generics: Vec::new(),
        params: vec![
            BackendParam {
                local_id: None,
                name: None,
                receiver: Some(nia_ids::ReceiverKind::RefReadOnly),
                passing_ty: i32_ptr_ty,
                local_ty: i32_ty,
                span,
            },
            BackendParam {
                local_id: None,
                name: None,
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span,
            },
        ],
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let main = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(3),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(body(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Call {
                callee: FunctionCallee::DynamicTraitMethod {
                    object_ty,
                    trait_id: TraitId::Source(trait_def),
                    method_id: method_def,
                    method_name: known::SHOW,
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    slot: 1,
                    params: Vec::new(),
                    return_type: i32_ty,
                    receiver_kind: nia_ids::ReceiverKind::RefReadOnly,
                    receiver: Box::new(FunctionExpr {
                        span,
                        ty: object_ty,
                        kind: FunctionExprKind::Null,
                    }),
                },
                args: Vec::new(),
            },
        })),
        span,
    };
    let secondary_method = BackendFunction {
        def_id: secondary_method_def,
        name: known::SHOW,
        link_name: None,
        generics: Vec::new(),
        params: vec![
            BackendParam {
                local_id: None,
                name: None,
                receiver: Some(nia_ids::ReceiverKind::RefReadOnly),
                passing_ty: bool_ptr_ty,
                local_ty: bool_ty,
                span,
            },
            BackendParam {
                local_id: None,
                name: None,
                receiver: None,
                passing_ty: bool_ty,
                local_ty: bool_ty,
                span,
            },
        ],
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let return_method = BackendFunction {
        def_id: return_method_def,
        name: known::SHOW,
        link_name: None,
        generics: Vec::new(),
        params: vec![
            BackendParam {
                local_id: None,
                name: None,
                receiver: Some(nia_ids::ReceiverKind::RefReadOnly),
                passing_ty: i32_ptr_ty,
                local_ty: i32_ty,
                span,
            },
            BackendParam {
                local_id: None,
                name: None,
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span,
            },
        ],
        return_type: bool_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let bad_abi = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(4),
        },
        name: sym("bad_abi"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(body(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Call {
                callee: FunctionCallee::DynamicTraitMethod {
                    object_ty,
                    trait_id: TraitId::Source(trait_def),
                    method_id: method_def,
                    method_name: known::SHOW,
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    slot: 0,
                    params: vec![i32_ty],
                    return_type: i32_ty,
                    receiver_kind: nia_ids::ReceiverKind::RefReadOnly,
                    receiver: Box::new(FunctionExpr {
                        span,
                        ty: object_ty,
                        kind: FunctionExprKind::Null,
                    }),
                },
                args: vec![FunctionExpr {
                    span,
                    ty: i32_ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                }],
            },
        })),
        span,
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                    (bool_ptr_ty, TypeLayout { size: 8, align: 8 }),
                    (object_ty, TypeLayout { size: 16, align: 8 }),
                    (child_object_ty, TypeLayout { size: 16, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![method, secondary_method, return_method, main, bad_abi],
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: vec![
                BackendTraitObjectVtable {
                    key: BackendTraitObjectVtableKey {
                        self_ty: i32_ty,
                        object_ty,
                    },
                    trait_id: TraitId::Source(trait_def),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    entries: vec![BackendTraitObjectVtableEntry {
                        trait_id: TraitId::Source(trait_def),
                        trait_args: Vec::new(),
                        trait_const_args: Vec::new(),
                        method_id: method_def,
                        method_name: known::SHOW,
                        slot: 0,
                        function: BackendTraitObjectVtableFunction::Function(method_def),
                    }],
                    span,
                },
                BackendTraitObjectVtable {
                    key: BackendTraitObjectVtableKey {
                        self_ty: bool_ty,
                        object_ty,
                    },
                    trait_id: TraitId::Source(trait_def),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    entries: vec![BackendTraitObjectVtableEntry {
                        trait_id: TraitId::Source(trait_def),
                        trait_args: Vec::new(),
                        trait_const_args: Vec::new(),
                        method_id: method_def,
                        method_name: known::SHOW,
                        slot: 0,
                        function: BackendTraitObjectVtableFunction::Function(secondary_method_def),
                    }],
                    span,
                },
                // This source table can reach `object_ty` through an upcast.
                // Its root-trait entry makes the relative object-view offset
                // non-zero, while the selected target has a malformed return
                // ABI. Direct tables above must not hide this candidate.
                BackendTraitObjectVtable {
                    key: BackendTraitObjectVtableKey {
                        self_ty: i32_ty,
                        object_ty: child_object_ty,
                    },
                    trait_id: TraitId::Source(child_trait_def),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    entries: vec![
                        BackendTraitObjectVtableEntry {
                            trait_id: TraitId::Source(child_trait_def),
                            trait_args: Vec::new(),
                            trait_const_args: Vec::new(),
                            method_id: method_def,
                            method_name: known::SHOW,
                            slot: 0,
                            function: BackendTraitObjectVtableFunction::Function(method_def),
                        },
                        BackendTraitObjectVtableEntry {
                            trait_id: TraitId::Source(trait_def),
                            trait_args: Vec::new(),
                            trait_const_args: Vec::new(),
                            method_id: method_def,
                            method_name: known::SHOW,
                            slot: 1,
                            function: BackendTraitObjectVtableFunction::Function(return_method_def),
                        },
                    ],
                    span,
                },
            ],
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "invalid vtable method slot"
        ),
        "{:?}",
        output.diagnostics
    );
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "parameter metadata does not match the vtable target signature"
        ),
        "{:?}",
        output.diagnostics
    );
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "return metadata does not match the vtable target signature"
        ),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_call_signatures_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let function_pointer_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let callable_ty = interner.intern(TyKind::Callable {
        is_readonly: true,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let span = Span::default();
    let target_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let method_id = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let instance_id = GlobalDefId {
        module_id,
        def_id: DefId(3),
    };
    let param = |ty| BackendParam {
        local_id: None,
        name: None,
        receiver: None,
        passing_ty: ty,
        local_ty: ty,
        span,
    };
    let call = |ty, callee, args| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Call { callee, args },
    };
    let null = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let integer = || FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Integer("1".to_string()),
    };
    let boolean = || FunctionExpr {
        span,
        ty: bool_ty,
        kind: FunctionExprKind::Bool(true),
    };
    let body = FunctionBody {
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
            ops: vec![
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::Function(target_id),
                    vec![boolean()],
                )),
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::FunctionInstance {
                        def_id: instance_id,
                        arg_module_id: module_id,
                        self_arg: None,
                        args: vec![i32_ty],
                        const_args: Vec::new(),
                    },
                    Vec::new(),
                )),
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::Method {
                        def_id: method_id,
                        arg_module_id: module_id,
                        self_arg: None,
                        args: Vec::new(),
                        const_args: Vec::new(),
                        receiver_kind: nia_ids::ReceiverKind::Ref,
                        receiver: Box::new(integer()),
                    },
                    vec![integer()],
                )),
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::Callable(Box::new(null(callable_ty))),
                    Vec::new(),
                )),
                FunctionOp::Expr(call(
                    bool_ty,
                    FunctionCallee::FunctionPointer(Box::new(null(function_pointer_ty))),
                    vec![integer()],
                )),
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::BuiltinMethod {
                        method: nia_function_ir::FunctionBuiltinMethod::SliceLen,
                        self_ty: i32_ty,
                        receiver: Box::new(integer()),
                    },
                    vec![integer()],
                )),
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::BuiltinOperator(nia_function_ir::FunctionBuiltinOperator {
                        trait_id: nia_ids::BuiltinTrait::Neg,
                        op: nia_function_ir::FunctionBuiltinOperatorOp::Unary(
                            nia_ast::UnaryOp::Neg,
                        ),
                    }),
                    vec![integer(), integer()],
                )),
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(integer()),
                span,
            },
        }],
        entry: FunctionBlockId(0),
        ty: i32_ty,
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                    (function_pointer_ty, TypeLayout { size: 8, align: 8 }),
                    (callable_ty, TypeLayout { size: 16, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![
                BackendFunction {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(0),
                    },
                    name: sym("main"),
                    link_name: None,
                    generics: Vec::new(),
                    params: Vec::new(),
                    return_type: i32_ty,
                    is_extern: false,
                    is_variadic: false,
                    attributes: Vec::new(),
                    local_names: Default::default(),
                    function_body: Some(body),
                    span,
                },
                BackendFunction {
                    def_id: target_id,
                    name: sym("target"),
                    link_name: None,
                    generics: Vec::new(),
                    params: vec![param(i32_ty)],
                    return_type: i32_ty,
                    is_extern: false,
                    is_variadic: false,
                    attributes: Vec::new(),
                    local_names: Default::default(),
                    function_body: None,
                    span,
                },
                BackendFunction {
                    def_id: method_id,
                    name: sym("method"),
                    link_name: None,
                    generics: Vec::new(),
                    params: vec![
                        BackendParam {
                            receiver: Some(nia_ids::ReceiverKind::RefReadOnly),
                            passing_ty: i32_ptr_ty,
                            local_ty: i32_ty,
                            ..param(i32_ptr_ty)
                        },
                        param(i32_ty),
                    ],
                    return_type: i32_ty,
                    is_extern: false,
                    is_variadic: false,
                    attributes: Vec::new(),
                    local_names: Default::default(),
                    function_body: None,
                    span,
                },
            ],
            function_instances: vec![BackendFunctionInstance {
                def_id: instance_id,
                name: sym("instance"),
                arg_module_id: module_id,
                self_arg: None,
                args: vec![i32_ty],
                const_args: Vec::new(),
                symbol: "instance_i32".to_string(),
                params: vec![param(i32_ty)],
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span,
            }],
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for message in [
        "function call has an invalid ABI contract: argument type",
        "function-instance call has an invalid ABI contract: argument count",
        "method call has an invalid ABI contract: receiver kind",
        "callable call has an invalid ABI contract: argument count",
        "function-pointer call has an invalid ABI contract: result type",
        "builtin-method call has an invalid ABI contract",
        "builtin-method call has an invalid ABI contract: len receiver type",
        "builtin-method call has an invalid ABI contract: len result type",
        "builtin-operator call has an invalid ABI contract",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_backend_ir_static_initializer_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let missing_global = GlobalDefId {
        module_id,
        def_id: DefId(9),
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (ptr_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
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
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("ptr"),
                link_name: None,
                ty: ptr_ty,
                is_let: true,
                is_extern: false,
                init: Some(StaticInit::AddrOfGlobal {
                    global: missing_global,
                    path: Vec::new(),
                }),
                span,
            }],
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("static initializer references missing global")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_initializer_field_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let field_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let missing_field = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
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
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: struct_id,
                name: sym("Point"),
                generics: Vec::new(),
                fields: vec![BackendField {
                    def_id: field_id,
                    name: sym("x"),
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
                    module_id,
                    def_id: DefId(3),
                },
                name: sym("point"),
                link_name: None,
                ty: struct_ty,
                is_let: true,
                is_extern: false,
                init: Some(StaticInit::Struct(vec![StaticFieldInit {
                    field: Some(missing_field),
                    value: StaticInit::Int(1.into()),
                }])),
                span,
            }],
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("static initializer references missing field")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_enum_variant_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let missing_variant = GlobalDefId {
        module_id,
        def_id: DefId(3),
    };
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: vec![BackendEnum {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("Mode"),
                backing_type: i32_ty,
                variants: vec![BackendEnumVariant {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(1),
                    },
                    name: sym("Known"),
                    value: Some(0),
                    payload: BackendEnumVariantPayload::Unit,
                    span,
                }],
                span,
            }],
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(2),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
                                kind: FunctionExprKind::EnumVariant {
                                    variant: missing_variant,
                                    fields: Vec::new(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("expression references missing enum variant")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_ir_missing_entry_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "backend IR contains invalid function IR"
        ),
        "{:?}",
        output.diagnostics
    );
    assert!(
        has_internal_diagnostic(
            &output.diagnostics,
            codes::INVALID_BACKEND_IR,
            "function entry block"
        ),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_ir_missing_successor_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: sym("main"),
                link_name: None,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
                is_extern: false,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
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
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("terminator references missing block")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_abi_param_local_mapping_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: vec![
            BackendParam {
                local_id: Some(LocalId(0)),
                name: Some(sym("left")),
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span,
            },
            BackendParam {
                local_id: Some(LocalId(0)),
                name: Some(sym("right")),
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span,
            },
        ],
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(FunctionBody {
            span,
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("left"),
                    kind: FunctionLocalKind::Param,
                    ty: i32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("right"),
                    kind: FunctionLocalKind::Param,
                    ty: i32_ty,
                    span,
                },
            ],
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "function parameters reference duplicate body local"
    ));
}

#[test]
fn validates_closure_abi_param_local_mapping_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let closure_id = nia_ids::ClosureId {
        owner: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        ordinal: 0,
    };
    let state_ty = interner.intern(TyKind::ClosureState {
        closure_id,
        captures: Vec::new(),
        params: vec![i32_ty, i32_ty],
        return_type: i32_ty,
    });
    let state_pointer_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: state_ty,
    });
    let entry = nia_backend_ir::BackendClosureEntry {
        key: nia_backend_ir::BackendClosureEntryKey {
            closure_id,
            owner: nia_backend_ir::BackendClosureEntryOwner::Source(closure_id.owner),
        },
        symbol: "main__closure_entry__ord__0".to_string(),
        abi: nia_backend_ir::BackendClosureEntryAbi {
            state_type: state_ty,
            state_pointer_type: state_pointer_ty,
            params: vec![i32_ty, i32_ty],
            return_type: i32_ty,
        },
        state_param: LocalId(0),
        params: vec![LocalId(1), LocalId(1)],
        local_names: Default::default(),
        function_body: FunctionBody {
            span,
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("state"),
                    kind: FunctionLocalKind::Param,
                    ty: state_pointer_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("left"),
                    kind: FunctionLocalKind::Param,
                    ty: i32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(2),
                    name: local_name("right"),
                    kind: FunctionLocalKind::Param,
                    ty: i32_ty,
                    span,
                },
            ],
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
                        kind: FunctionExprKind::Integer("0".to_string()),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        },
        span,
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (state_ty, TypeLayout { size: 0, align: 1 }),
                    (state_pointer_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: vec![entry],
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry ABI parameters reference duplicate body local"
    ));
}

#[test]
fn validates_closure_entry_call_contract_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let main_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let closure_id = nia_ids::ClosureId {
        owner: main_id,
        ordinal: 0,
    };
    let state_ty = interner.intern(TyKind::ClosureState {
        closure_id,
        captures: Vec::new(),
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let state_pointer_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: state_ty,
    });
    let integer = || FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Integer("0".to_string()),
    };
    let main = BackendFunction {
        def_id: main_id,
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                ops: vec![
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: bool_ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::ClosureEntry {
                                closure_id,
                                state: Box::new(integer()),
                            },
                            args: Vec::new(),
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::ClosureEntry {
                                closure_id: nia_ids::ClosureId {
                                    owner: main_id,
                                    ordinal: 1,
                                },
                                state: Box::new(FunctionExpr {
                                    span,
                                    ty: state_pointer_ty,
                                    kind: FunctionExprKind::Null,
                                }),
                            },
                            args: vec![integer()],
                        },
                    }),
                ],
                terminator: FunctionTerminator::Tail {
                    value: Some(integer()),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        }),
        span,
    };
    let entry = nia_backend_ir::BackendClosureEntry {
        key: nia_backend_ir::BackendClosureEntryKey {
            closure_id,
            owner: nia_backend_ir::BackendClosureEntryOwner::Source(main_id),
        },
        symbol: "main__closure_entry__ord__0".to_string(),
        abi: nia_backend_ir::BackendClosureEntryAbi {
            state_type: state_ty,
            state_pointer_type: state_pointer_ty,
            params: vec![i32_ty],
            return_type: i32_ty,
        },
        state_param: LocalId(0),
        params: vec![LocalId(1)],
        local_names: Default::default(),
        function_body: FunctionBody {
            span,
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("state"),
                    kind: FunctionLocalKind::Param,
                    ty: state_pointer_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("value"),
                    kind: FunctionLocalKind::Param,
                    ty: i32_ty,
                    span,
                },
            ],
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
                    value: Some(integer()),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: i32_ty,
        },
        span,
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (state_ty, TypeLayout { size: 0, align: 1 }),
                    (state_pointer_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![main],
            function_instances: Vec::new(),
            closure_entries: vec![entry],
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for message in [
        "closure-entry call has an invalid ABI contract: state pointer type",
        "closure-entry call has an invalid ABI contract: argument count",
        "closure-entry call has an invalid ABI contract: result type",
        "closure-entry call has an invalid ABI contract: call references a missing generated entry",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_function_ir_local_storage_type_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let span = Span::default();
    let bool_expr = || FunctionExpr {
        span,
        ty: bool_ty,
        kind: FunctionExprKind::Bool(true),
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(FunctionBody {
            span,
            locals: vec![FunctionLocal {
                id: LocalId(0),
                name: local_name("value"),
                kind: FunctionLocalKind::MutableBinding,
                ty: i32_ty,
                span,
            }],
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: vec![
                    FunctionOp::Binding(nia_function_ir::FunctionBinding {
                        local_id: LocalId(0),
                        name: local_name("value"),
                        ty: bool_ty,
                        value: Some(bool_expr()),
                        is_let: false,
                    }),
                    FunctionOp::Binding(nia_function_ir::FunctionBinding {
                        local_id: LocalId(0),
                        name: local_name("value"),
                        ty: i32_ty,
                        value: Some(bool_expr()),
                        is_let: false,
                    }),
                    FunctionOp::StoreLocal {
                        local_id: LocalId(0),
                        value: bool_expr(),
                        span,
                    },
                ],
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (bool_ty, TypeLayout { size: 1, align: 1 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for message in [
        "binding type does not match its body local",
        "binding initializer type does not match its binding",
        "stored value type does not match its body local",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_backend_ir_static_function_address_refs_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let fn_ptr_ty = interner.intern(TyKind::FunctionPointer {
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
    });
    let span = Span::default();
    let missing_function = GlobalDefId {
        module_id,
        def_id: DefId(9),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (fn_ptr_ty, TypeLayout { size: 8, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![BackendGlobal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(0),
            },
            name: sym("ptr"),
            link_name: None,
            ty: fn_ptr_ty,
            is_let: true,
            is_extern: false,
            init: Some(StaticInit::AddrOfFunction {
                function: missing_function,
                args: Vec::new(),
            }),
            span,
        }],
        Vec::new(),
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("static initializer references missing function")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_static_address_path_shape_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let source_global = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (ptr_ty, TypeLayout { size: 8, align: 8 }),
            ],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        vec![
            BackendGlobal {
                def_id: source_global,
                name: sym("value"),
                link_name: None,
                ty: i32_ty,
                is_let: false,
                is_extern: false,
                init: Some(StaticInit::Int(0.into())),
                span,
            },
            BackendGlobal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                name: sym("ptr"),
                link_name: None,
                ty: ptr_ty,
                is_let: true,
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

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("indexes non-array type")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_aggregate_literal_field_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let field_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let missing_field = GlobalDefId {
        module_id,
        def_id: DefId(9),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(2),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: struct_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
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
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        vec![BackendStruct {
            def_id: struct_id,
            name: sym("Box"),
            generics: Vec::new(),
            fields: vec![BackendField {
                def_id: field_id,
                name: sym("value"),
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

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("aggregate literal references missing field")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_missing_local_place_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("place local references missing local")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_unresolved_trait_method_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let trait_id = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let method_id = GlobalDefId {
        module_id,
        def_id: DefId(11),
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                                method_name: known::VALUE,
                                self_ty: i32_ty,
                                trait_args: Vec::new(),
                                trait_const_args: Vec::new(),
                                args: Vec::new(),
                                receiver_kind: nia_ids::ReceiverKind::Value,
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unresolved trait method")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_unresolved_builtin_place_method_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
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
                                trait_id: BuiltinTrait::SliceMut,
                                method: BuiltinTraitMethod::SliceMut,
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
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
            structs: Vec::new(),
            unions: Vec::new(),
            enums: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("unresolved builtin place method")),
        "{:?}",
        output.diagnostics
    );
}
