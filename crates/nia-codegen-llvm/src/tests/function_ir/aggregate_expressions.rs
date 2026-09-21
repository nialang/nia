use super::*;


#[test]
fn validates_aggregate_products_with_structurally_equal_const_args() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let def_id = GlobalDefId {
        module_id,
        def_id: DefId(40),
    };
    let field_id = GlobalDefId {
        module_id,
        def_id: DefId(41),
    };
    let signed_arg = ConstGenericArg {
        ty: i32_ty,
        value: ConstGenericValue::Int(IntConst::signed_bits(7)),
    };
    let unsigned_arg = ConstGenericArg {
        ty: i32_ty,
        value: ConstGenericValue::Int(IntConst::unsigned(7)),
    };
    let nominal_ty = interner.test_intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![signed_arg.clone()],
    });
    let span = Span::default();
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            symbol_package_identity: "test/package@0".into(),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (nominal_ty, TypeLayout { size: 8, align: 4 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: vec![(
                    nia_backend_ir::BackendStructInstanceKey {
                        def_id,
                        args: Vec::new(),
                        const_args: vec![signed_arg],
                    },
                    StructLayout {
                        layout: TypeLayout { size: 8, align: 4 },
                        fields: vec![FieldLayout {
                            def_id: field_id.def_id,
                            offset: 0,
                            layout: TypeLayout { size: 8, align: 4 },
                        }],
                    },
                )],
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                is_extern: false,
                def_id,
                name: sym("ConstPacket"),
                generics: vec![sym("N")],
                fields: vec![BackendField {
                    def_id: field_id,
                    name: sym("value"),
                    ty: i32_ty,
                    span,
                }],
                span,
            }],
            struct_instances: vec![nia_backend_ir::BackendStructInstance {
                is_extern: false,
                def_id,
                name: sym("ConstPacket7"),
                args: Vec::new(),
                const_args: vec![unsigned_arg],
                symbol: "const_packet_7".to_string(),
                fields: vec![BackendField {
                    def_id: field_id,
                    name: sym("value"),
                    ty: i32_ty,
                    span,
                }],
                span,
            }],
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "field layout does not match its declared type"
    ));
}

#[test]
fn emits_bitmask_with_32_bit_usize_result() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let boolx16_ty = interner.test_intern(TyKind::Vector {
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: usize_ty,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
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
fn validates_switch_case_constants_and_uniqueness_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i8_ty = interner.test_primitive(PrimitiveTy::I8);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let case = |kind| FunctionExpr {
        span,
        ty: i8_ty,
        kind,
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(FunctionBody {
            span,
            locals: vec![FunctionLocal {
                id: LocalId(0),
                name: local_name("value"),
                kind: FunctionLocalKind::ImmutableBinding,
                ty: i8_ty,
                span,
            }],
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
                    terminator: FunctionTerminator::Switch {
                        target: case(FunctionExprKind::Local(LocalId(0))),
                        arms: vec![
                            FunctionSwitchArm {
                                pattern: case(FunctionExprKind::Local(LocalId(0))),
                                target: FunctionBlockId(1),
                            },
                            FunctionSwitchArm {
                                pattern: case(FunctionExprKind::Integer("-1".to_string())),
                                target: FunctionBlockId(1),
                            },
                            FunctionSwitchArm {
                                pattern: case(FunctionExprKind::BuiltinValue(
                                    nia_function_ir::FunctionBuiltinValue::Int(
                                        nia_ty::IntConst::unsigned(255),
                                    ),
                                )),
                                target: FunctionBlockId(1),
                            },
                        ],
                        default: Some(FunctionBlockId(1)),
                        fallback: FunctionBlockId(1),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(1),
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
            BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (i8_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
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
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    for message in [
        "switch arm pattern is not a compile-time integer constant",
        "switch contains duplicate case values",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_literal_payloads_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i8_ty = interner.test_primitive(PrimitiveTy::I8);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let char_array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: char_ty,
    });
    let span = Span::default();
    let expr = |ty, kind| FunctionOp::Expr(FunctionExpr { span, ty, kind });
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("main"),
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
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
                    expr(i32_ty, FunctionExprKind::Integer("invalid".to_string())),
                    expr(i8_ty, FunctionExprKind::Integer("128".to_string())),
                    expr(f32_ty, FunctionExprKind::Float("invalid".to_string())),
                    expr(f32_ty, FunctionExprKind::Float("1e999".to_string())),
                    expr(char_ty, FunctionExprKind::Char(0xd800)),
                    expr(
                        u8_ty,
                        FunctionExprKind::ByteChar("not-a-byte-char".to_string()),
                    ),
                    expr(char_array_ty, FunctionExprKind::String(vec![0xd800])),
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
    drop(interner);

    let output = emit_owned_llvm_ir(
        single_module_program(
            module_id,
            BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (char_ty, TypeLayout { size: 4, align: 4 }),
                    (f32_ty, TypeLayout { size: 4, align: 4 }),
                    (i8_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (u8_ty, TypeLayout { size: 1, align: 1 }),
                    (char_array_ty, TypeLayout { size: 4, align: 4 }),
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
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    for message in [
        "integer literal has an invalid type contract: spelling is invalid",
        "integer literal has an invalid type contract: value is outside its target type",
        "float literal has an invalid type contract: spelling is invalid",
        "float literal has an invalid type contract: value is outside its target type",
        "char literal has an invalid type contract: value is not a Unicode scalar",
        "byte char literal has an invalid type contract: spelling is invalid",
        "string literal has an invalid type contract: value contains an invalid Unicode scalar",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_projection_and_field_initializer_types_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let tuple_ty = interner.test_intern(TyKind::Tuple(vec![i32_ty]));
    let array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: i32_ty,
    });
    let slice_i32_ty = interner.test_intern(TyKind::Slice {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_slice_i32_ty = interner.test_intern(TyKind::Slice {
        is_readonly: true,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
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
    let struct_ty = interner.test_intern(TyKind::Nominal {
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type,
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
            interner.test_intern(TyKind::Slice {
                is_readonly: false,
                elem: bool_ty,
            }),
            FunctionExpr {
                span,
                ty: interner.test_intern(TyKind::Slice {
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
            14,
            "bad_index_bound",
            i32_ty,
            FunctionExpr {
                span,
                ty: i32_ty,
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
                        ty: f32_ty,
                        kind: FunctionExprKind::Float("0.0".to_string()),
                    }),
                },
            },
        ),
        function(
            15,
            "bad_index_target",
            i32_ty,
            FunctionExpr {
                span,
                ty: i32_ty,
                kind: FunctionExprKind::Index {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty: bool_ty,
                        kind: FunctionExprKind::Bool(true),
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
            is_extern: false,
            def_id: struct_id,
            name: sym("Box"),
            generics: Vec::new(),
            fields: vec![BackendField {
                def_id: field_id,
                name: sym("value"),
                ty: i32_ty,
                span,
            }],
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
        "index expression is not integer-like",
        "index target is not indexable storage",
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
