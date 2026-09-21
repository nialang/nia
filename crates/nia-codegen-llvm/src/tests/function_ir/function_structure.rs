use super::*;

#[test]
fn rejects_enum_variant_with_foreign_owner_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let foreign_module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let foreign_variant = GlobalDefId {
        module_id: foreign_module_id,
        def_id: DefId(1),
    };
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
                    def_id: foreign_variant,
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
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Tail {
                            value: Some(FunctionExpr {
                                span,
                                ty: i32_ty,
                                kind: FunctionExprKind::EnumVariant {
                                    variant: foreign_variant,
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
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("enum variant")
                && diagnostic
                    .summary
                    .contains("does not belong to its enum module")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_function_ir_missing_entry_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
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
        .try_into()
        .expect("build backend modules"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
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
        .try_into()
        .expect("build backend modules"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let module_mangle = nia_mangle::MangleModuleId::from_normalized_source_path("main");
    let closure_id = nia_ids::ClosureId {
        owner: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        ordinal: 0,
    };
    let state_ty = interner.test_intern(TyKind::ClosureState {
        closure_id,
        captures: Vec::new(),
        params: vec![i32_ty, i32_ty],
        return_type: i32_ty,
    });
    let state_pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: state_ty,
    });
    let malformed_state_pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let entry = nia_backend_ir::BackendClosureEntry {
        key: nia_backend_ir::BackendClosureEntryKey {
            closure_id,
            owner: nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(
                nia_function_ir::FunctionInstanceKey {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(1),
                    },
                    arg_module_id: module_id,
                    self_arg: None,
                    args: Vec::new(),
                    const_args: Vec::new(),
                },
            ),
        },
        symbol: nia_mangle::mangle_derived_symbol_canonical(
            "test/package@0",
            module_mangle,
            "wrong_closure_owner",
            "wrong_closure_owner",
            MangleSymbolKind::Function,
            std::iter::empty(),
        )
        .expect("mangle malformed closure owner"),
        abi: nia_backend_ir::BackendClosureEntryAbi {
            state_type: i32_ty,
            state_pointer_type: malformed_state_pointer_ty,
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
                    ty: malformed_state_pointer_ty,
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
    let mut missing_owner_entry = entry.clone();
    missing_owner_entry.key.owner = nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(
        nia_function_ir::FunctionInstanceKey {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(2),
            },
            arg_module_id: module_id,
            self_arg: None,
            args: Vec::new(),
            const_args: Vec::new(),
        },
    );
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
                    (state_ty, TypeLayout { size: 0, align: 1 }),
                    (state_pointer_ty, TypeLayout { size: 8, align: 8 }),
                    (malformed_state_pointer_ty, TypeLayout { size: 8, align: 8 }),
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
            function_instances: vec![BackendFunctionInstance {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                name: sym("owner_instance"),
                arg_module_id: module_id,
                self_arg: None,
                args: Vec::new(),
                const_args: Vec::new(),
                symbol: "owner_instance".to_string(),
                params: Vec::new(),
                return_type: i32_ty,
                linkage: BackendLinkage::Nia,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span,
            }],
            closure_entries: vec![entry.clone(), missing_owner_entry, entry],
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
        "closure entry ABI parameters reference duplicate body local"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry state type does not match its identity and ABI signature"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry body contains unmapped parameter local"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry owner does not match its source closure identity"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry owner does not resolve to a backend function"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "closure entry is not published with its owning backend function"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "backend module contains a duplicate closure entry identity"
    ));
}

#[test]
fn validates_closure_entry_call_and_view_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let main_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let closure_id = nia_ids::ClosureId {
        owner: main_id,
        ordinal: 0,
    };
    let state_ty = interner.test_intern(TyKind::ClosureState {
        closure_id,
        captures: Vec::new(),
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let state_pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: state_ty,
    });
    let callable_ty = interner.test_intern(TyKind::Callable {
        is_readonly: true,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let mutable_callable_ty = interner.test_intern(TyKind::Callable {
        is_readonly: false,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let wrong_callable_ty = interner.test_intern(TyKind::Callable {
        is_readonly: true,
        params: vec![bool_ty],
        return_type: i32_ty,
    });
    let closure_fn_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let variadic_closure_fn_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: true,
    });
    let integer = || FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Integer("0".to_string()),
    };
    let main = BackendFunction {
        def_id: main_id,
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
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: wrong_callable_ty,
                        kind: FunctionExprKind::CallableCoercion {
                            state: Box::new(FunctionExpr {
                                span,
                                ty: state_pointer_ty,
                                kind: FunctionExprKind::Null,
                            }),
                            closure_id,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: mutable_callable_ty,
                        kind: FunctionExprKind::CallableCoercion {
                            state: Box::new(FunctionExpr {
                                span,
                                ty: state_pointer_ty,
                                kind: FunctionExprKind::Null,
                            }),
                            closure_id,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::CallableCoercion {
                            state: Box::new(FunctionExpr {
                                span,
                                ty: state_pointer_ty,
                                kind: FunctionExprKind::Null,
                            }),
                            closure_id,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: variadic_closure_fn_ty,
                        kind: FunctionExprKind::ClosureFunctionPointer { closure_id },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: closure_fn_ty,
                        kind: FunctionExprKind::ClosureFunctionPointer {
                            closure_id: nia_ids::ClosureId {
                                owner: main_id,
                                ordinal: 1,
                            },
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: callable_ty,
                        kind: FunctionExprKind::CallableCoercion {
                            state: Box::new(FunctionExpr {
                                span,
                                ty: state_pointer_ty,
                                kind: FunctionExprKind::Null,
                            }),
                            closure_id,
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
        symbol: nia_mangle::mangle_derived_symbol_canonical(
            "test/package@0",
            nia_mangle::MangleModuleId::from_normalized_source_path("main"),
            "closure_fixture",
            "closure_fixture",
            MangleSymbolKind::ClosureEntry,
            ["ordinal:0".to_string()],
        )
        .expect("mangle closure fixture"),
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
            symbol_package_identity: "test/package@0".into(),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (state_ty, TypeLayout { size: 0, align: 1 }),
                    (state_pointer_ty, TypeLayout { size: 8, align: 8 }),
                    (callable_ty, TypeLayout { size: 16, align: 8 }),
                    (mutable_callable_ty, TypeLayout { size: 16, align: 8 }),
                    (wrong_callable_ty, TypeLayout { size: 16, align: 8 }),
                    (closure_fn_ty, TypeLayout { size: 8, align: 8 }),
                    (variadic_closure_fn_ty, TypeLayout { size: 8, align: 8 }),
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
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for message in [
        "closure-entry call has an invalid ABI contract: state pointer type",
        "closure-entry call has an invalid ABI contract: argument count",
        "closure-entry call has an invalid ABI contract: result type",
        "closure-entry call has an invalid ABI contract: call references a missing generated entry",
        "callable signature does not match closure state",
        "mutable callable has a readonly state pointer",
        "result is not callable",
        "closure function-pointer result is not a non-variadic function pointer",
        "generated closure entry is missing",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
}
