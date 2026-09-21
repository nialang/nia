use super::*;

#[test]
fn validates_backend_ir_missing_array_length_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let span = Span::default();
    let len_id = GlobalConstExprId {
        module_id,
        const_expr_id: ConstExprId(0),
    };
    let elem = interner.test_primitive(PrimitiveTy::U8);
    let array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(len_id),
        elem,
    });
    let mut const_eval = BackendConstFacts::default();
    const_eval.array_lengths.insert(len_id, 4);
    let mut module = BackendModule {
        id: module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        symbol_package_identity: "test/package@0".into(),
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
            linkage: BackendLinkage::Nia,
            ty: array_ty,
            is_let: false,
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
    let program = BackendProgram::new(vec![module]).expect("build backend program");

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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let box_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let box_ty = interner.test_intern(TyKind::Nominal {
        def_id: box_id,
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
                linkage: BackendLinkage::Nia,
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
            .any(|diagnostic| diagnostic.summary.contains("has no ABI layout")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_error_type_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let error_ty = interner.error();
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
                name: sym("take"),
                linkage: BackendLinkage::Nia,
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
            .any(|diagnostic| diagnostic.summary.contains("is error")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn validates_backend_ir_propagation_contract_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let optional_i32_ty = interner.test_intern(TyKind::Optional { elem: i32_ty });
    let source_result_ty = interner.test_intern(TyKind::ErrorUnion {
        error: i32_ty,
        value: i32_ty,
    });
    let target_result_ty = interner.test_intern(TyKind::ErrorUnion {
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
            linkage: BackendLinkage::Nia,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let foreign_module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let foreign_store = nia_ty::TypeStore::new().expect("create type store");
    let foreign_ty = foreign_store
        .append_for_module(module_id)
        .test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let callee_id = GlobalDefId {
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
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::FunctionInstance {
                                        def_id: callee_id,
                                        arg_module_id: module_id,
                                        self_arg: None,
                                        args: vec![i32_ty],
                                        const_args: vec![ConstGenericArg {
                                            ty: foreign_ty,
                                            value: ConstGenericValue::ConstExpr(
                                                GlobalConstExprId {
                                                    module_id: foreign_module_id,
                                                    const_expr_id: ConstExprId(0),
                                                },
                                            ),
                                        }],
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
            "call references missing function instance"
        ),
        "{:?}",
        output.diagnostics
    );
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "type belongs to a different compilation session"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "const argument expression"
    ));
}

#[test]
fn validates_indexed_function_instances_with_equivalent_type_args() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let canonical_struct_ty = interner.test_intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let equivalent_struct_ty = interner.test_intern(TyKind::Nominal {
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
            symbol_package_identity: "test/package@0".into(),
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
                linkage: BackendLinkage::Nia,
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
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("@make_marker()"));
}

#[test]
fn validates_backend_ir_vtable_structure_and_function_refs_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let object_ty = interner.test_intern(TyKind::TraitObject {
        is_readonly: true,
        trait_id: TraitId::Source(GlobalDefId {
            module_id,
            def_id: DefId(0),
        }),
        trait_args: Vec::new(),
        trait_const_args: vec![ConstGenericArg {
            ty: i32_ty,
            value: ConstGenericValue::Int(IntConst::unsigned(3)),
        }],
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
                trait_args: Vec::new(),
                trait_const_args: vec![ConstGenericArg {
                    ty: i32_ty,
                    value: ConstGenericValue::Int(IntConst::signed(3)),
                }],
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
        .try_into()
        .expect("build backend modules"),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic
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
    let bool_ptr_ty = interner.test_intern(TyKind::Pointer {
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
    let object_ty = interner.test_intern(TyKind::TraitObject {
        is_readonly: true,
        trait_id: TraitId::Source(trait_def),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: Vec::new(),
    });
    let child_object_ty = interner.test_intern(TyKind::TraitObject {
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
        linkage: BackendLinkage::Nia,
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
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
        linkage: BackendLinkage::Nia,
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
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let return_method = BackendFunction {
        def_id: return_method_def,
        name: known::SHOW,
        linkage: BackendLinkage::Nia,
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
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
            symbol_package_identity: "test/package@0".into(),
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
fn emits_const_only_extern_method_instances_with_c_abi() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let receiver_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let const_arg = ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::Int(IntConst::unsigned(4)),
    };
    let span = Span::default();
    let main_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let method_id = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let param = BackendParam {
        local_id: Some(LocalId(0)),
        name: None,
        receiver: Some(nia_ids::ReceiverKind::RefReadOnly),
        passing_ty: receiver_ty,
        local_ty: receiver_ty,
        span,
    };
    let call = FunctionExpr {
        span,
        ty: i32_ty,
        kind: FunctionExprKind::Call {
            callee: FunctionCallee::Method {
                def_id: method_id,
                arg_module_id: module_id,
                self_arg: None,
                args: Vec::new(),
                const_args: vec![const_arg.clone()],
                receiver_kind: nia_ids::ReceiverKind::RefReadOnly,
                receiver: Box::new(FunctionExpr {
                    span,
                    ty: receiver_ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
            },
            args: Vec::new(),
        },
    };
    let body = FunctionBody {
        span,
        locals: vec![FunctionLocal {
            id: LocalId(0),
            name: local_name("receiver"),
            kind: FunctionLocalKind::Param,
            ty: receiver_ty,
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
            ops: Vec::new(),
            terminator: FunctionTerminator::Tail {
                value: Some(call),
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
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (usize_ty, TypeLayout { size: 8, align: 8 }),
                    (receiver_ty, TypeLayout { size: 8, align: 8 }),
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
                def_id: main_id,
                name: sym("main"),
                linkage: BackendLinkage::Nia,
                generics: Vec::new(),
                params: vec![BackendParam {
                    local_id: Some(LocalId(0)),
                    name: Some(sym("receiver")),
                    receiver: None,
                    passing_ty: receiver_ty,
                    local_ty: receiver_ty,
                    span,
                }],
                return_type: i32_ty,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: Some(body),
                span,
            }],
            function_instances: vec![BackendFunctionInstance {
                def_id: method_id,
                name: sym("const_method"),
                arg_module_id: module_id,
                self_arg: None,
                args: Vec::new(),
                const_args: vec![const_arg],
                symbol: "const_method_4".to_string(),
                params: vec![param],
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

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @const_method_4(ptr)"), "{ir}");
    assert!(ir.contains("call i32 @const_method_4(ptr "), "{ir}");
}
