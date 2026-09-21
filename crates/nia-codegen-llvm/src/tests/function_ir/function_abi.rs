use super::*;

#[test]
fn emits_function_body_from_function_ir_when_available() {
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
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 2"), "{ir}");
}

#[test]
fn scopes_template_local_promotions_to_function_instances() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let array_ty = interner.test_intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::ConstValue(1),
        elem: usize_ty,
    });
    let pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: array_ty,
    });
    let function_span = Span::new(1, 100);
    let allocation_span = Span::new(40, 50);
    let def_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let name = sym("promoted");
    let module_mangle = nia_mangle::MangleModuleId::from_normalized_source_path("main");
    let const_arg = |value: u128| ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::Int(IntConst::unsigned(value)),
    };
    let instance_symbol = |arg: &ConstGenericArg| {
        nia_mangle::mangle_instance_symbol_canonical_with_context(
            nia_mangle::MangleInstance::new(
                "test/package@0",
                module_mangle,
                nia_mangle::stable_definition_key(def_id),
                nia_mangle::mangle_symbol_id(name),
                &[],
                std::slice::from_ref(arg),
                MangleSymbolKind::Function,
            )
            .expect("build mangle instance"),
            &type_store,
            nia_mangle::MangleResolvers::new(
                |_| module_mangle,
                |_| "missing".to_string(),
                |_| None,
            ),
            Some(module_mangle),
        )
        .expect("mangle instance symbol")
    };
    let body = |value: u128| FunctionBody {
        span: function_span,
        locals: Vec::new(),
        scopes: vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span: function_span,
        }],
        blocks: vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span: function_span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span: allocation_span,
                    ty: pointer_ty,
                    kind: FunctionExprKind::StaticArrayPointer {
                        allocation: nia_function_ir::PromotedAllocationId::new(
                            module_id,
                            allocation_span,
                        ),
                        array: Box::new(FunctionExpr {
                            span: allocation_span,
                            ty: array_ty,
                            kind: FunctionExprKind::ArrayLiteral {
                                elems: FunctionArrayElements::List(vec![FunctionExpr {
                                    span: allocation_span,
                                    ty: usize_ty,
                                    kind: FunctionExprKind::Integer(value.to_string()),
                                }]),
                            },
                        }),
                        is_readonly: true,
                    },
                }),
                span: function_span,
            },
        }],
        entry: FunctionBlockId(0),
        ty: pointer_ty,
    };
    let instance = |value: u128| {
        let arg = const_arg(value);
        BackendFunctionInstance {
            def_id,
            name,
            arg_module_id: module_id,
            self_arg: None,
            args: Vec::new(),
            const_args: vec![arg.clone()],
            symbol: instance_symbol(&arg),
            params: Vec::new(),
            return_type: pointer_ty,
            linkage: BackendLinkage::Nia,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: Some(body(value)),
            span: function_span,
        }
    };
    let program = BackendProgram::new(vec![BackendModule {
        id: module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        symbol_package_identity: "test/package@0".into(),
        name: "main".to_string(),
        const_eval: BackendConstFacts::default(),
        layouts: BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (usize_ty, TypeLayout { size: 8, align: 8 }),
                (array_ty, TypeLayout { size: 8, align: 8 }),
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
        globals: Vec::new(),
        global_instances: Vec::new(),
        functions: vec![BackendFunction {
            def_id,
            name,
            linkage: BackendLinkage::Nia,
            generics: vec![sym("N")],
            params: Vec::new(),
            return_type: pointer_ty,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span: function_span,
        }],
        function_instances: vec![instance(1), instance(2)],
        closure_entries: Vec::new(),
        trait_object_vtables: Vec::new(),
        generic_instantiations: Vec::new(),
    }])
    .expect("build backend program");

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_eq!(
        ir.matches("linkonce_odr constant [1 x i64]").count(),
        2,
        "{ir}"
    );
    assert_eq!(
        count_canonical_symbols(ir, '@', "promoted_allocation", MangleSymbolKind::Global),
        4,
        "{ir}"
    );
    assert!(
        ir.contains("linkonce_odr constant [1 x i64] [i64 1]"),
        "{ir}"
    );
    assert!(
        ir.contains("linkonce_odr constant [1 x i64] [i64 2]"),
        "{ir}"
    );
}

#[test]
fn rejects_conflicting_promoted_allocation_initializers() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let array_ty = interner.test_intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::ConstValue(1),
        elem: usize_ty,
    });
    let pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: array_ty,
    });
    let result_ty = interner.test_intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::ConstValue(2),
        elem: pointer_ty,
    });
    let span = Span::new(1, 10);
    let allocation = nia_function_ir::PromotedAllocationId::new(module_id, span);
    let promoted = |value: u128| FunctionExpr {
        span,
        ty: pointer_ty,
        kind: FunctionExprKind::StaticArrayPointer {
            allocation,
            array: Box::new(FunctionExpr {
                span,
                ty: array_ty,
                kind: FunctionExprKind::ArrayLiteral {
                    elems: FunctionArrayElements::List(vec![FunctionExpr {
                        span,
                        ty: usize_ty,
                        kind: FunctionExprKind::Integer(value.to_string()),
                    }]),
                },
            }),
            is_readonly: true,
        },
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
        return_type: result_ty,
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
                        ty: result_ty,
                        kind: FunctionExprKind::ArrayLiteral {
                            elems: FunctionArrayElements::List(vec![promoted(1), promoted(2)]),
                        },
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: result_ty,
        }),
        span,
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (usize_ty, TypeLayout { size: 8, align: 8 }),
                (array_ty, TypeLayout { size: 8, align: 8 }),
                (pointer_ty, TypeLayout { size: 8, align: 8 }),
                (result_ty, TypeLayout { size: 16, align: 8 }),
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "promoted allocation identity was reused with a different initializer"
    ));
}

#[test]
fn validates_function_return_runtime_layout_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let opaque_ty = interner.test_intern(TyKind::Opaque);
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("opaque_return"),
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: opaque_ty,
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
fn validates_variadic_function_declarations_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
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
    };
    let function = |def_id, name, function_body: Option<FunctionBody>| BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        },
        name: sym(name),
        linkage: if function_body.is_some() {
            BackendLinkage::ExternExport {
                symbol: name.to_string(),
            }
        } else {
            BackendLinkage::ExternImport {
                symbol: name.to_string(),
            }
        },
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: true,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body,
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
            functions: vec![function(0, "variadic_definition", Some(body))],
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
        "extern variadic function requires at least one fixed parameter"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "extern variadic function definition is not supported"
    ));
}

#[test]
fn validates_naked_function_attribute_before_llvm() {
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
        name: sym("invalid_naked"),
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
        attributes: vec![nia_backend_ir::BackendFunctionAttribute::Naked],
        local_names: Default::default(),
        function_body: None,
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
        "backend IR `naked` attribute is only valid on extern function definitions"
    ));
}

#[test]
fn validates_external_linkage_contract_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = |def_id, name, linkage, generics| BackendFunction {
        def_id: GlobalDefId { module_id, def_id },
        name: sym(name),
        linkage,
        generics,
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let global = |def_id, name, external_symbol: &str| BackendGlobal {
        def_id: GlobalDefId { module_id, def_id },
        name: sym(name),
        linkage: BackendLinkage::ExternImport {
            symbol: external_symbol.to_string(),
        },
        ty: i32_ty,
        is_let: false,
        init: None,
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
        vec![
            global(DefId(2), "empty_link", ""),
            global(DefId(3), "nul_link", "bad\0name"),
        ],
        vec![
            function(
                DefId(0),
                "generic_extern",
                BackendLinkage::ExternImport {
                    symbol: "generic_extern".into(),
                },
                vec![sym("T")],
            ),
            function(
                DefId(1),
                "local_template",
                BackendLinkage::ExternExport {
                    symbol: "forged_external_name".into(),
                },
                vec![sym("T")],
            ),
        ],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "backend IR extern function cannot have generic parameters",
        "backend IR exported function requires a body",
        "backend IR global external symbol must not be empty or contain NUL",
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
                .contains("must not be empty or contain NUL"))
            .count(),
        2,
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_extern_abi_types_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let tuple_ty = interner.test_intern(TyKind::Tuple(vec![i32_ty]));
    let optional_ty = interner.test_intern(TyKind::Optional { elem: i32_ty });
    let generic_array_ty = interner.test_intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::GenericParam(sym("N")),
        elem: i32_ty,
    });
    let variadic_function_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: i32_ty,
        is_variadic: true,
    });
    let span = Span::default();
    let param = |ty| BackendParam {
        local_id: None,
        name: None,
        receiver: None,
        passing_ty: ty,
        local_ty: ty,
        span,
    };
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("malformed_extern"),
        linkage: BackendLinkage::ExternImport {
            symbol: "malformed_extern".to_string(),
        },
        generics: Vec::new(),
        params: vec![param(bool_ty), param(variadic_function_ty)],
        return_type: tuple_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let field_id = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let item = BackendStruct {
        is_extern: true,
        def_id: struct_id,
        name: sym("MalformedExternStruct"),
        generics: Vec::new(),
        fields: vec![
            BackendField {
                def_id: field_id,
                name: sym("value"),
                ty: optional_ty,
                span,
            },
            BackendField {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(4),
                },
                name: sym("values"),
                ty: generic_array_ty,
                span,
            },
        ],
        span,
    };
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(3),
        },
        name: sym("malformed_extern_global"),
        linkage: BackendLinkage::ExternImport {
            symbol: "malformed_extern_global".to_string(),
        },
        ty: char_ty,
        is_let: false,
        init: None,
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
        vec![item],
        Vec::new(),
        vec![global],
        vec![function],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "extern parameter cannot use `bool` directly",
        "extern parameter cannot use variadic function pointer",
        "extern function pointer parameter cannot use `bool` directly",
        "extern return type cannot use tuple by value",
        "extern global cannot use `char` directly",
        "extern struct field cannot use optional by value",
        "extern struct field cannot use an unresolved array length",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_function_instance_abi_metadata_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let foreign_module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let param = BackendParam {
        local_id: None,
        name: None,
        receiver: None,
        passing_ty: i32_ty,
        local_ty: i32_ty,
        span,
    };
    let template = BackendFunction {
        def_id,
        name: sym("template"),
        linkage: BackendLinkage::Nia,
        generics: vec![sym("T"), sym("U"), sym("V")],
        params: vec![param.clone()],
        return_type: i32_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let instance = BackendFunctionInstance {
        def_id,
        name: sym("forged_template"),
        arg_module_id: module_id,
        self_arg: None,
        args: vec![i32_ty],
        const_args: vec![ConstGenericArg {
            ty: i32_ty,
            value: ConstGenericValue::ConstExpr(GlobalConstExprId {
                module_id: foreign_module_id,
                const_expr_id: ConstExprId(0),
            }),
        }],
        symbol: "bad\0function_instance".to_string(),
        params: vec![BackendParam {
            receiver: Some(nia_ids::ReceiverKind::Value),
            ..param
        }],
        return_type: i32_ty,
        linkage: BackendLinkage::ExternImport {
            symbol: "forged_template".into(),
        },
        is_variadic: true,
        attributes: vec![nia_backend_ir::BackendFunctionAttribute::Naked],
        local_names: Default::default(),
        function_body: None,
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
            functions: vec![template],
            function_instances: vec![instance],
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
    for expected in [
        "function instance generic argument arity does not match its source template",
        "function instance name does not match its source template",
        "function instance parameter metadata does not match its source template",
        "function instance linkage does not match its source template",
        "function instance variadic flag does not match its source template",
        "function instance attributes do not match its source template",
        "const argument expression",
        "backend IR function instance symbol must not be empty or contain NUL",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_aggregate_instance_abi_metadata_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let foreign_type_store = nia_ty::TypeStore::new().expect("create type store");
    let foreign_interner = foreign_type_store.append_for_module(module_id);
    let foreign_i32_ty = foreign_interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let union_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
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
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                is_extern: false,
                def_id: struct_id,
                name: sym("StructTemplate"),
                generics: vec![sym("T")],
                fields: Vec::new(),
                span,
            }],
            struct_instances: vec![
                nia_backend_ir::BackendStructInstance {
                    is_extern: true,
                    def_id: struct_id,
                    name: sym("ForgedStructTemplate"),
                    args: vec![foreign_i32_ty],
                    const_args: Vec::new(),
                    symbol: "bad\0struct_instance".to_string(),
                    fields: vec![BackendField {
                        def_id: GlobalDefId {
                            module_id,
                            def_id: DefId(2),
                        },
                        name: sym("forged"),
                        ty: i32_ty,
                        span,
                    }],
                    span,
                },
                nia_backend_ir::BackendStructInstance {
                    is_extern: false,
                    def_id: struct_id,
                    name: sym("StructTemplate"),
                    args: vec![i32_ty],
                    const_args: Vec::new(),
                    symbol: "forged_struct_symbol".to_string(),
                    fields: Vec::new(),
                    span,
                },
            ],
            unions: vec![BackendUnion {
                is_extern: false,
                def_id: union_id,
                name: sym("UnionTemplate"),
                generics: vec![sym("T"), sym("U")],
                fields: Vec::new(),
                span,
            }],
            union_instances: vec![nia_backend_ir::BackendUnionInstance {
                is_extern: true,
                def_id: union_id,
                name: sym("ForgedUnionTemplate"),
                args: vec![i32_ty],
                const_args: Vec::new(),
                symbol: "bad\0union_instance".to_string(),
                fields: vec![BackendField {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(3),
                    },
                    name: sym("forged"),
                    ty: i32_ty,
                    span,
                }],
                span,
            }],
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

    drop(foreign_interner);
    drop(foreign_type_store);
    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "type belongs to a different compilation session",
        "struct instance extern flag does not match its source template",
        "struct instance name does not match its source template",
        "struct instance field metadata does not match its source template",
        "union instance extern flag does not match its source template",
        "union instance generic argument arity does not match its source template",
        "union instance name does not match its source template",
        "union instance field metadata does not match its source template",
        "backend IR struct instance symbol must not be empty or contain NUL",
        "backend IR union instance symbol must not be empty or contain NUL",
        "backend IR struct instance symbol does not match its instance identity",
        "backend IR union instance symbol does not match its instance identity",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_global_instance_metadata_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let foreign_type_store = nia_ty::TypeStore::new().expect("create type store");
    let foreign_interner = foreign_type_store.append_for_module(module_id);
    let foreign_i32_ty = foreign_interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
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
            globals: vec![BackendGlobal {
                def_id,
                name: sym("template"),
                linkage: BackendLinkage::ExternImport {
                    symbol: "template".to_string(),
                },
                ty: i32_ty,
                is_let: true,
                init: None,
                span,
            }],
            global_instances: vec![
                nia_backend_ir::BackendGlobalInstance {
                    def_id,
                    name: sym("forged_template"),
                    arg_module_id: module_id,
                    args: vec![foreign_i32_ty],
                    const_args: Vec::new(),
                    symbol: "bad\0global_instance".to_string(),
                    ty: i32_ty,
                    is_let: false,
                    init: Some(StaticInit::Zero),
                    span,
                },
                nia_backend_ir::BackendGlobalInstance {
                    def_id,
                    name: sym("template"),
                    arg_module_id: module_id,
                    args: vec![i32_ty],
                    const_args: Vec::new(),
                    symbol: "forged_global_symbol".to_string(),
                    ty: i32_ty,
                    is_let: true,
                    init: None,
                    span,
                },
            ],
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }]
        .try_into()
        .expect("build backend modules"),
    };

    drop(foreign_interner);
    drop(foreign_type_store);
    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "type belongs to a different compilation session",
        "global instance cannot materialize an extern source global",
        "global instance name does not match its source template",
        "global instance mutability does not match its source template",
        "global instance initializer presence does not match its source template",
        "backend IR global instance symbol must not be empty or contain NUL",
        "backend IR global instance symbol does not match its instance identity",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}
