use super::*;

#[test]
fn validates_backend_ir_call_signatures_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let function_pointer_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let wrong_function_params_ty = interner.test_intern(TyKind::FunctionPointer {
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
    });
    let wrong_function_return_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: bool_ty,
        is_variadic: false,
    });
    let wrong_function_variadic_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: true,
    });
    let callable_ty = interner.test_intern(TyKind::Callable {
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
                        receiver: Box::new(FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        }),
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
                FunctionOp::Expr(call(
                    i32_ty,
                    FunctionCallee::BuiltinOperator(nia_function_ir::FunctionBuiltinOperator {
                        trait_id: nia_ids::BuiltinTrait::Add,
                        op: nia_function_ir::FunctionBuiltinOperatorOp::Unary(
                            nia_ast::UnaryOp::Neg,
                        ),
                    }),
                    vec![integer()],
                )),
                FunctionOp::Expr(call(
                    bool_ty,
                    FunctionCallee::BuiltinOperator(nia_function_ir::FunctionBuiltinOperator {
                        trait_id: nia_ids::BuiltinTrait::Neg,
                        op: nia_function_ir::FunctionBuiltinOperatorOp::Binary(
                            nia_ast::BinaryOp::Add,
                        ),
                    }),
                    vec![integer(), integer()],
                )),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: bool_ty,
                    kind: FunctionExprKind::Function(target_id),
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: i32_ptr_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::RefReadOnly,
                        expr: Box::new(FunctionExpr {
                            span,
                            ty: function_pointer_ty,
                            kind: FunctionExprKind::Function(target_id),
                        }),
                    },
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: wrong_function_params_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::RefReadOnly,
                        expr: Box::new(FunctionExpr {
                            span,
                            ty: function_pointer_ty,
                            kind: FunctionExprKind::Function(target_id),
                        }),
                    },
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: wrong_function_params_ty,
                    kind: FunctionExprKind::Function(target_id),
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: wrong_function_return_ty,
                    kind: FunctionExprKind::Function(target_id),
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: wrong_function_variadic_ty,
                    kind: FunctionExprKind::Function(target_id),
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: wrong_function_params_ty,
                    kind: FunctionExprKind::FunctionInstance {
                        def_id: instance_id,
                        arg_module_id: module_id,
                        self_arg: None,
                        args: vec![i32_ty],
                        const_args: Vec::new(),
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
    };
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
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                    (function_pointer_ty, TypeLayout { size: 8, align: 8 }),
                    (wrong_function_params_ty, TypeLayout { size: 8, align: 8 }),
                    (wrong_function_return_ty, TypeLayout { size: 8, align: 8 }),
                    (wrong_function_variadic_ty, TypeLayout { size: 8, align: 8 }),
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
                    linkage: BackendLinkage::Nia,
                    generics: Vec::new(),
                    params: Vec::new(),
                    return_type: i32_ty,
                    is_variadic: false,
                    attributes: Vec::new(),
                    local_names: Default::default(),
                    function_body: Some(body),
                    span,
                },
                BackendFunction {
                    def_id: target_id,
                    name: sym("target"),
                    linkage: BackendLinkage::Nia,
                    generics: Vec::new(),
                    params: vec![param(i32_ty)],
                    return_type: i32_ty,
                    is_variadic: false,
                    attributes: Vec::new(),
                    local_names: Default::default(),
                    function_body: None,
                    span,
                },
                BackendFunction {
                    def_id: method_id,
                    name: sym("method"),
                    linkage: BackendLinkage::Nia,
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
                linkage: BackendLinkage::Nia,
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
        .try_into()
        .expect("build backend modules"),
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
        "builtin-operator call has an invalid ABI contract: operator trait",
        "backend IR operator has an invalid contract: binary result type",
        "backend IR operator has an invalid contract: function reference result is not a function pointer",
        "backend IR operator has an invalid contract: function reference result type does not match its function item",
        "function value has an invalid signature contract: value type",
        "function value has an invalid signature contract: parameter types",
        "function value has an invalid signature contract: return type",
        "function value has an invalid signature contract: variadic flag",
        "function-instance value has an invalid signature contract: parameter types",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}
#[test]
fn validates_backend_ir_inline_asm_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let unit_ty = interner.test_intern(TyKind::Tuple(Vec::new()));
    let tuple_ty = interner.test_intern(TyKind::Tuple(vec![i32_ty]));
    let span = Span::default();
    let scalar_local = LocalId(0);
    let aggregate_local = LocalId(1);
    let local_expr = |local, ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Local(local),
    };
    let local_place = |local, ty| FunctionPlace {
        span,
        ty,
        base: FunctionPlaceBase::Local(local),
        elems: Vec::new(),
    };
    let asm = FunctionExpr {
        span,
        ty: bool_ty,
        kind: FunctionExprKind::InlineAsm(FunctionInlineAsm {
            code: "mov eax, eax".to_string(),
            inputs: vec![FunctionAsmInput {
                constraint: "r,~{memory}".to_string(),
                value: local_expr(aggregate_local, tuple_ty),
                span,
            }],
            outputs: vec![
                FunctionAsmOutput {
                    constraint: "r".to_string(),
                    place: local_place(scalar_local, i32_ty),
                    span,
                },
                FunctionAsmOutput {
                    constraint: "=r".to_string(),
                    place: local_place(aggregate_local, tuple_ty),
                    span,
                },
            ],
            clobbers: vec!["memory},~{cc".to_string()],
            options: Vec::new(),
        }),
    };
    let body = FunctionBody {
        span,
        locals: vec![
            FunctionLocal {
                id: scalar_local,
                name: local_name("scalar"),
                kind: FunctionLocalKind::ImmutableBinding,
                ty: i32_ty,
                span,
            },
            FunctionLocal {
                id: aggregate_local,
                name: local_name("aggregate"),
                kind: FunctionLocalKind::MutableBinding,
                ty: tuple_ty,
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
            ops: vec![FunctionOp::Expr(asm)],
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
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (bool_ty, TypeLayout { size: 1, align: 1 }),
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (unit_ty, TypeLayout { size: 0, align: 1 }),
                (tuple_ty, TypeLayout { size: 4, align: 4 }),
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
        vec![BackendFunction {
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
            function_body: Some(body),
            span,
        }],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for message in [
        "inline assembly has an invalid contract: expression result type is not unit",
        "inline assembly has an invalid contract: input operand type is not scalar",
        "inline assembly has an invalid contract: input constraint is not canonical",
        "inline assembly has an invalid contract: output storage is not writable",
        "inline assembly has an invalid contract: output constraint is not canonical",
        "inline assembly has an invalid contract: output operand type is not scalar",
        "inline assembly has an invalid contract: clobber name contains constraint syntax",
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let ptr_ty = interner.test_intern(TyKind::Pointer {
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
            symbol_package_identity: "test/package@0".into(),
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
                linkage: BackendLinkage::Nia,
                ty: ptr_ty,
                is_let: true,
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
        .try_into()
        .expect("build backend modules"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
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
    let struct_ty = interner.test_intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let union_id = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let union_field_a = GlobalDefId {
        module_id,
        def_id: DefId(11),
    };
    let union_field_b = GlobalDefId {
        module_id,
        def_id: DefId(12),
    };
    let union_ty = interner.test_intern(TyKind::Nominal {
        def_id: union_id,
        args: Vec::new(),
        const_args: Vec::new(),
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
                    (struct_ty, TypeLayout { size: 4, align: 4 }),
                    (union_ty, TypeLayout { size: 4, align: 4 }),
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
                unions: vec![(
                    union_id,
                    StructLayout {
                        layout: TypeLayout { size: 4, align: 4 },
                        fields: vec![FieldLayout {
                            def_id: union_field_a.def_id,
                            offset: 0,
                            layout: TypeLayout { size: 4, align: 4 },
                        }],
                    },
                )],
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
            unions: vec![BackendUnion {
                def_id: union_id,
                name: sym("Value"),
                generics: Vec::new(),
                fields: vec![
                    BackendField {
                        def_id: union_field_a,
                        name: sym("integer"),
                        ty: i32_ty,
                        span,
                    },
                    BackendField {
                        def_id: union_field_b,
                        name: sym("other"),
                        ty: i32_ty,
                        span,
                    },
                ],
                is_extern: false,
                span,
            }],
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![
                BackendGlobal {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(3),
                    },
                    name: sym("point"),
                    linkage: BackendLinkage::Nia,
                    ty: struct_ty,
                    is_let: true,
                    init: Some(StaticInit::Struct(vec![StaticFieldInit {
                        field: Some(missing_field),
                        value: StaticInit::Int(1.into()),
                    }])),
                    span,
                },
                BackendGlobal {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(4),
                    },
                    name: sym("duplicate_point"),
                    linkage: BackendLinkage::Nia,
                    ty: struct_ty,
                    is_let: true,
                    init: Some(StaticInit::Struct(vec![
                        StaticFieldInit {
                            field: Some(field_id),
                            value: StaticInit::Int(1.into()),
                        },
                        StaticFieldInit {
                            field: Some(field_id),
                            value: StaticInit::Int(2.into()),
                        },
                    ])),
                    span,
                },
                BackendGlobal {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(5),
                    },
                    name: sym("missing_point"),
                    linkage: BackendLinkage::Nia,
                    ty: struct_ty,
                    is_let: true,
                    init: Some(StaticInit::Struct(Vec::new())),
                    span,
                },
                BackendGlobal {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(6),
                    },
                    name: sym("two_values"),
                    linkage: BackendLinkage::Nia,
                    ty: union_ty,
                    is_let: true,
                    init: Some(StaticInit::Struct(vec![
                        StaticFieldInit {
                            field: Some(union_field_a),
                            value: StaticInit::Int(1.into()),
                        },
                        StaticFieldInit {
                            field: Some(union_field_b),
                            value: StaticInit::Int(2.into()),
                        },
                    ])),
                    span,
                },
            ],
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
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("static initializer references missing field")),
        "{:?}",
        output.diagnostics
    );
    for message in [
        "struct static initializer duplicates a field",
        "struct static initializer is missing a field",
        "union static initializer must initialize exactly one field",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}
