use super::*;

#[test]
fn validates_function_ir_local_storage_type_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let fn_ptr_ty = interner.test_intern(TyKind::FunctionPointer {
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
            linkage: BackendLinkage::Nia,
            ty: fn_ptr_ty,
            is_let: true,
            init: Some(StaticInit::AddrOfFunction {
                function: missing_function,
                args: Vec::new(),
                const_args: Vec::new(),
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
fn validates_static_function_address_signatures_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let wrong_params_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let wrong_variadic_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: bool_ty,
        is_variadic: true,
    });
    let function_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let span = Span::default();
    let globals = [wrong_params_ty, wrong_variadic_ty]
        .into_iter()
        .enumerate()
        .map(|(index, ty)| BackendGlobal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId((index as u64) + 1),
            },
            name: sym(if index == 0 {
                "wrong_params"
            } else {
                "wrong_variadic"
            }),
            linkage: BackendLinkage::Nia,
            ty,
            is_let: true,
            init: Some(StaticInit::AddrOfFunction {
                function: function_id,
                args: Vec::new(),
                const_args: Vec::new(),
            }),
            span,
        })
        .collect();
    let function = BackendFunction {
        def_id: function_id,
        name: sym("target"),
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: vec![BackendParam {
            local_id: None,
            name: None,
            receiver: None,
            passing_ty: i32_ty,
            local_ty: i32_ty,
            span,
        }],
        return_type: bool_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    drop(interner);

    let output = emit_owned_llvm_ir(
        single_module_program(
            module_id,
            BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (wrong_params_ty, TypeLayout { size: 8, align: 8 }),
                    (wrong_variadic_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            Vec::new(),
            Vec::new(),
            globals,
            vec![function],
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    for message in [
        "function address parameter types do not match its target",
        "function address return type does not match its target",
        "function address variadic flag does not match its target",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_static_function_address_instance_with_structurally_equal_args() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let nominal_def = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let signed_arg = ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::Int(IntConst::signed_bits(7)),
    };
    let unsigned_arg = ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::Int(IntConst::unsigned(7)),
    };
    let declared_arg_ty = interner.test_intern(TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![signed_arg],
    });
    let referenced_arg_ty = interner.test_intern(TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![unsigned_arg],
    });
    let function_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let pointer_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: i32_ty,
        is_variadic: false,
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
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (usize_ty, TypeLayout { size: 8, align: 8 }),
                    (pointer_ty, TypeLayout { size: 8, align: 8 }),
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
                    def_id: DefId(1),
                },
                name: sym("callback"),
                linkage: BackendLinkage::Nia,
                ty: pointer_ty,
                is_let: true,
                init: Some(StaticInit::AddrOfFunction {
                    function: function_id,
                    args: vec![referenced_arg_ty],
                    const_args: Vec::new(),
                }),
                span,
            }],
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: vec![BackendFunctionInstance {
                def_id: function_id,
                name: sym("target"),
                arg_module_id: module_id,
                self_arg: None,
                args: vec![declared_arg_ty],
                const_args: Vec::new(),
                symbol: "target_7".to_string(),
                params: vec![BackendParam {
                    local_id: None,
                    name: None,
                    receiver: None,
                    passing_ty: i32_ty,
                    local_ty: i32_ty,
                    span,
                }],
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "function address parameter types do not match its target"
    ));
}

#[test]
fn validates_backend_ir_static_address_path_shape_before_llvm() {
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
                linkage: BackendLinkage::Nia,
                ty: i32_ty,
                is_let: false,
                init: Some(StaticInit::Int(0.into())),
                span,
            },
            BackendGlobal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                name: sym("ptr"),
                linkage: BackendLinkage::Nia,
                ty: ptr_ty,
                is_let: true,
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
fn validates_static_global_address_pointee_and_mutability_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let readonly_bool_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_ty,
    });
    let mutable_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let source_global = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let span = Span::default();
    let address_global = |def_id, name, ty| BackendGlobal {
        def_id: GlobalDefId { module_id, def_id },
        name: sym(name),
        linkage: BackendLinkage::Nia,
        ty,
        is_let: true,
        init: Some(StaticInit::AddrOfGlobal {
            global: source_global,
            path: Vec::new(),
        }),
        span,
    };
    let globals = vec![
        BackendGlobal {
            def_id: source_global,
            name: sym("value"),
            linkage: BackendLinkage::Nia,
            ty: i32_ty,
            is_let: true,
            init: Some(StaticInit::Int(0.into())),
            span,
        },
        address_global(DefId(1), "wrong_pointee", readonly_bool_ptr_ty),
        address_global(DefId(2), "wrong_mutability", mutable_i32_ptr_ty),
    ];
    drop(interner);

    let output = emit_owned_llvm_ir(
        single_module_program(
            module_id,
            BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (readonly_bool_ptr_ty, TypeLayout { size: 8, align: 8 }),
                    (mutable_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            Vec::new(),
            Vec::new(),
            globals,
            Vec::new(),
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    for message in [
        "global address pointee type does not match its path",
        "global address exposes immutable storage as mutable",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_backend_ir_missing_aggregate_literal_field_before_llvm() {
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
        def_id: DefId(9),
    };
    let struct_ty = interner.test_intern(TyKind::Nominal {
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: struct_ty,
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
                        ty: struct_ty,
                        kind: FunctionExprKind::StructLiteral {
                            def_id: missing_field,
                            fields: vec![FunctionFieldInit {
                                field: Some(field_id),
                                name: "value".to_string(),
                                value: FunctionExpr {
                                    span,
                                    ty: i32_ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                },
                                span,
                            }],
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: struct_ty,
                        kind: FunctionExprKind::StructLiteral {
                            def_id: struct_id,
                            fields: vec![
                                FunctionFieldInit {
                                    field: Some(field_id),
                                    name: "value".to_string(),
                                    value: FunctionExpr {
                                        span,
                                        ty: i32_ty,
                                        kind: FunctionExprKind::Integer("1".to_string()),
                                    },
                                    span,
                                },
                                FunctionFieldInit {
                                    field: Some(field_id),
                                    name: "value".to_string(),
                                    value: FunctionExpr {
                                        span,
                                        ty: i32_ty,
                                        kind: FunctionExprKind::Integer("2".to_string()),
                                    },
                                    span,
                                },
                            ],
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: struct_ty,
                        kind: FunctionExprKind::UnionLiteral {
                            def_id: struct_id,
                            field: Box::new(FunctionFieldInit {
                                field: Some(field_id),
                                name: "value".to_string(),
                                value: FunctionExpr {
                                    span,
                                    ty: i32_ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                },
                                span,
                            }),
                        },
                    }),
                ],
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
    for message in [
        "struct literal has an invalid type contract: definition does not match expression type",
        "struct literal has an invalid type contract: definition has the wrong aggregate kind",
        "struct literal has an invalid type contract: fields do not initialize each declared field exactly once",
        "union literal has an invalid type contract: definition has the wrong aggregate kind",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_backend_ir_missing_local_place_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
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
fn validates_backend_ir_place_type_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let i32_array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: i32_ty,
    });
    let bool_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_ty,
    });
    let i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let local_place = |local_id, ty, elems| FunctionPlace {
        span,
        ty,
        base: FunctionPlaceBase::Local(local_id),
        elems,
    };
    let addr = |ty, place| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::AddrOf(place),
        })
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
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("value"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: i32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("values"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: i32_array_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(2),
                    name: local_name("mutable_pointer"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: i32_ptr_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(3),
                    name: local_name("readonly_pointer"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: readonly_i32_ptr_ty,
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
                ops: vec![
                    addr(i32_ty, local_place(LocalId(0), i32_ty, Vec::new())),
                    addr(
                        i32_ptr_ty,
                        FunctionPlace {
                            span,
                            ty: i32_ty,
                            base: FunctionPlaceBase::Deref(Box::new(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            })),
                            elems: Vec::new(),
                        },
                    ),
                    addr(
                        bool_ptr_ty,
                        local_place(
                            LocalId(1),
                            bool_ty,
                            vec![nia_function_ir::FunctionPlaceElem::Index(Box::new(
                                FunctionExpr {
                                    span,
                                    ty: f32_ty,
                                    kind: FunctionExprKind::Float("0.0".to_string()),
                                },
                            ))],
                        ),
                    ),
                    addr(
                        i32_ptr_ty,
                        local_place(
                            LocalId(0),
                            i32_ty,
                            vec![nia_function_ir::FunctionPlaceElem::TupleField(0)],
                        ),
                    ),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: bool_ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    // Reading mutable pointer storage through a readonly view is
                    // the one qualifier coercion represented directly on local IR.
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: readonly_i32_ptr_ty,
                        kind: FunctionExprKind::Local(LocalId(2)),
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ptr_ty,
                        kind: FunctionExprKind::Local(LocalId(3)),
                    }),
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
                (bool_ty, TypeLayout { size: 1, align: 1 }),
                (f32_ty, TypeLayout { size: 4, align: 4 }),
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (i32_array_ty, TypeLayout { size: 4, align: 4 }),
                (bool_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
                (readonly_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
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
        "address-of result is not a pointer",
        "deref base is not a pointer",
        "index is not an integer",
        "result type does not match the selected storage",
        "tuple projection target is not a tuple",
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
                .contains("local value has an invalid type contract"))
            .count(),
        2,
        "{:?}",
        output.diagnostics
    );
}
