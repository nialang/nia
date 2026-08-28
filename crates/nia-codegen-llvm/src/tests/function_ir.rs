// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_backend_ir::BackendEnumVariantPayload;
use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput,
    FunctionAtomic, FunctionBitIntrinsicOp, FunctionInlineAsm, FunctionMemoryIntrinsic,
    FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource, FunctionSliceRange,
    FunctionSwitchArm,
};
use nia_ty::{ConstGenericArg, ConstGenericValue, IntConst};

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
fn scopes_template_local_promotions_to_function_instances() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let array_ty = interner.intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::ConstValue(1),
        elem: usize_ty,
    });
    let pointer_ty = interner.intern(TyKind::Pointer {
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
        let symbol = nia_mangle::mangle_instance_symbol_id(
            def_id,
            name,
            &[],
            std::slice::from_ref(arg),
            &type_store,
            nia_mangle::MangleResolvers::new(
                |_| module_mangle,
                |_| "missing".to_string(),
                |_| None,
            ),
        );
        format!("{symbol}__ctx_s{:016x}", stable_hash("main"))
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
            is_extern: false,
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
            link_name: None,
            generics: vec![sym("N")],
            params: Vec::new(),
            return_type: pointer_ty,
            is_extern: false,
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
    }]);

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_eq!(
        ir.matches("linkonce_odr constant [1 x i64]").count(),
        2,
        "{ir}"
    );
    assert_eq!(ir.matches("__o").count(), 4, "{ir}");
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let array_ty = interner.intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::ConstValue(1),
        elem: usize_ty,
    });
    let pointer_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: array_ty,
    });
    let result_ty = interner.intern(TyKind::Array {
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
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: result_ty,
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
fn validates_variadic_function_declarations_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
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
    let function = |def_id, name, function_body| BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        },
        name: sym(name),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: true,
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
        .into(),
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
        name: sym("invalid_naked"),
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
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
fn validates_external_link_name_contract_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = |def_id, name, link_name, generics, is_extern| BackendFunction {
        def_id: GlobalDefId { module_id, def_id },
        name: sym(name),
        link_name,
        generics,
        params: Vec::new(),
        return_type: i32_ty,
        is_extern,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let global = |def_id, name, link_name: &str| BackendGlobal {
        def_id: GlobalDefId { module_id, def_id },
        name: sym(name),
        link_name: Some(link_name.to_string()),
        ty: i32_ty,
        is_let: false,
        is_extern: true,
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
            function(DefId(0), "generic_extern", None, vec![sym("T")], true),
            function(
                DefId(1),
                "local_template",
                Some("forged_external_name".to_string()),
                vec![sym("T")],
                false,
            ),
        ],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "backend IR extern function requires an external link name",
        "backend IR extern function cannot have generic parameters",
        "backend IR non-extern function cannot publish an external link name",
        "backend IR global link name must not be empty or contain NUL",
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let tuple_ty = interner.intern(TyKind::Tuple(vec![i32_ty]));
    let optional_ty = interner.intern(TyKind::Optional { elem: i32_ty });
    let generic_array_ty = interner.intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::GenericParam(sym("N")),
        elem: i32_ty,
    });
    let variadic_function_ty = interner.intern(TyKind::FunctionPointer {
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
        link_name: Some("malformed_extern".to_string()),
        generics: Vec::new(),
        params: vec![param(bool_ty), param(variadic_function_ty)],
        return_type: tuple_ty,
        is_extern: true,
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
        is_extern: true,
        span,
    };
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(3),
        },
        name: sym("malformed_extern_global"),
        link_name: Some("malformed_extern_global".to_string()),
        ty: char_ty,
        is_let: false,
        is_extern: true,
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
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
        link_name: None,
        generics: vec![sym("T"), sym("U")],
        params: vec![param.clone()],
        return_type: i32_ty,
        is_extern: false,
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
        const_args: Vec::new(),
        symbol: "bad\0function_instance".to_string(),
        params: vec![BackendParam {
            receiver: Some(nia_ids::ReceiverKind::Value),
            ..param
        }],
        return_type: i32_ty,
        is_extern: true,
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
        .into(),
    };

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "function instance generic argument arity does not match its source template",
        "function instance name does not match its source template",
        "function instance parameter metadata does not match its source template",
        "function instance extern flag does not match its source template",
        "function instance variadic flag does not match its source template",
        "function instance attributes do not match its source template",
        "backend IR function instance symbol must not be empty or contain NUL",
        "backend IR function instance symbol does not match its instance identity",
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let foreign_type_store = nia_ty::TypeStore::new();
    let foreign_interner = foreign_type_store.append_for_module(module_id);
    let foreign_i32_ty = foreign_interner.primitive(PrimitiveTy::I32);
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
                def_id: struct_id,
                name: sym("StructTemplate"),
                generics: vec![sym("T")],
                fields: Vec::new(),
                is_extern: false,
                span,
            }],
            struct_instances: vec![
                nia_backend_ir::BackendStructInstance {
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
                    is_extern: true,
                    span,
                },
                nia_backend_ir::BackendStructInstance {
                    def_id: struct_id,
                    name: sym("StructTemplate"),
                    args: vec![i32_ty],
                    const_args: Vec::new(),
                    symbol: "forged_struct_symbol".to_string(),
                    fields: Vec::new(),
                    is_extern: false,
                    span,
                },
            ],
            unions: vec![BackendUnion {
                def_id: union_id,
                name: sym("UnionTemplate"),
                generics: vec![sym("T"), sym("U")],
                fields: Vec::new(),
                is_extern: false,
                span,
            }],
            union_instances: vec![nia_backend_ir::BackendUnionInstance {
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
                is_extern: true,
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
        .into(),
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let foreign_type_store = nia_ty::TypeStore::new();
    let foreign_interner = foreign_type_store.append_for_module(module_id);
    let foreign_i32_ty = foreign_interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
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
            globals: vec![BackendGlobal {
                def_id,
                name: sym("template"),
                link_name: Some("template".to_string()),
                ty: i32_ty,
                is_let: true,
                is_extern: true,
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
        .into(),
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

#[test]
fn rejects_ordinary_definition_with_foreign_module_owner_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let foreign_module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: foreign_module_id,
            def_id: DefId(0),
        },
        name: sym("foreign_owner"),
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
        "function definition"
    ));
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("does not belong to its backend module")
    }));
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
fn emits_static_arrays_of_zero_vectors() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let vector_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(2),
        elem: vector_ty,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("vectors"),
        link_name: None,
        ty: array_ty,
        is_let: true,
        is_extern: false,
        init: Some(StaticInit::Array(vec![StaticInit::Zero, StaticInit::Zero])),
        span: Span::default(),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![
                (
                    vector_ty,
                    TypeLayout {
                        size: 16,
                        align: 16,
                    },
                ),
                (
                    array_ty,
                    TypeLayout {
                        size: 32,
                        align: 16,
                    },
                ),
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
    assert_eq!(output.modules.len(), 1);
    assert!(output.modules[0].ir.contains("[2 x <4 x i32>]"));
}

#[test]
fn rejects_malformed_static_vector_lanes_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let vector_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::U8,
        lanes: 2,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_vector"),
        link_name: None,
        ty: vector_ty,
        is_let: true,
        is_extern: false,
        init: Some(StaticInit::Vector(vec![StaticInit::Bool(true)])),
        span: Span::default(),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(vector_ty, TypeLayout { size: 2, align: 2 })],
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
        "vector static initializer has 1 lanes"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "vector static initializer lane does not match"
    ));
}

#[test]
fn rejects_malformed_static_tuple_elements_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let tuple_ty = interner.intern(TyKind::Tuple(vec![i32_ty, bool_ty]));
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_tuple"),
        link_name: None,
        ty: tuple_ty,
        is_let: true,
        is_extern: false,
        init: Some(StaticInit::Tuple(vec![StaticInit::Bool(true)])),
        span: Span::default(),
    };
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout::LP64,
            types: vec![(tuple_ty, TypeLayout { size: 8, align: 4 })],
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
        "tuple static initializer has 1 elements"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "bool initializer target is not bool"
    ));
}

#[test]
fn validates_static_scalar_initializer_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let vector_i32_ty = interner.intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let pointer_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let char_array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: char_ty,
    });
    let span = Span::default();
    let global = |index, ty, init| BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(index),
        },
        name: sym(&format!("global_{index}")),
        link_name: None,
        ty,
        is_let: true,
        is_extern: false,
        init: Some(init),
        span,
    };
    let globals = vec![
        global(0, i32_ty, StaticInit::Bool(true)),
        global(1, pointer_ty, StaticInit::Int(1.into())),
        global(2, char_ty, StaticInit::Char(0xd800)),
        global(3, i32_ty, StaticInit::Byte(0)),
        global(4, f32_ty, StaticInit::Float("invalid".to_string())),
        global(5, f32_ty, StaticInit::Float("1e999".to_string())),
        global(6, i32_ty, StaticInit::NullPtr),
        global(7, char_array_ty, StaticInit::Chars(vec![0xd800])),
        global(8, u8_ty, StaticInit::Bytes(vec![0])),
        global(9, bool_ty, StaticInit::Int(nia_ty::IntConst::unsigned(2))),
        global(10, char_ty, StaticInit::Int(nia_ty::IntConst::signed(-1))),
        global(
            11,
            i32_ty,
            StaticInit::Int(nia_ty::IntConst::unsigned(1_u128 << 32)),
        ),
        global(
            12,
            vector_i32_ty,
            StaticInit::Vector(vec![
                StaticInit::Int(nia_ty::IntConst::unsigned(1_u128 << 32)),
                StaticInit::Zero,
                StaticInit::Zero,
                StaticInit::Zero,
            ]),
        ),
        global(
            13,
            i32_ty,
            StaticInit::Int(nia_ty::IntConst::signed_bits(1_u128 << 31)),
        ),
    ];
    drop(interner);

    let output = emit_owned_llvm_ir(
        single_module_program(
            module_id,
            BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: vec![
                    (bool_ty, TypeLayout { size: 1, align: 1 }),
                    (char_ty, TypeLayout { size: 4, align: 4 }),
                    (f32_ty, TypeLayout { size: 4, align: 4 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (u8_ty, TypeLayout { size: 1, align: 1 }),
                    (
                        vector_i32_ty,
                        TypeLayout {
                            size: 16,
                            align: 16,
                        },
                    ),
                    (pointer_ty, TypeLayout { size: 8, align: 8 }),
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
            globals,
            Vec::new(),
        ),
        type_store,
    );
    assert!(output.modules.is_empty());
    for message in [
        "bool initializer target is not bool",
        "integer initializer target is not integer-like",
        "integer initializer value is outside its target type",
        "char initializer is not a Unicode scalar",
        "byte initializer target is not u8",
        "float initializer spelling or range is invalid",
        "null pointer initializer target is not a pointer",
        "char string contains an invalid Unicode scalar",
        "byte string initializer target is not u8 array",
        "vector static initializer lane does not match its primitive element type",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, message),
            "missing `{message}` in {:?}",
            output.diagnostics
        );
    }
    let out_of_range_integer_count = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.summary == "backend IR static initializer has an invalid scalar contract: integer initializer value is outside its target type"
        })
        .count();
    assert_eq!(out_of_range_integer_count, 3);
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
fn rejects_out_of_range_builtin_usize_before_llvm() {
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
        name: sym("invalid_usize"),
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
                            nia_function_ir::FunctionBuiltinValue::Usize(u64::MAX),
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "usize constant value is outside its target pointer width"
    ));
}

#[test]
fn rejects_invalid_target_layout_without_published_type_layouts() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let program = single_module_program(
        module_id,
        BackendLayouts {
            target: nia_layout::TargetDataLayout {
                pointer_size: 4,
                pointer_align: 2,
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
        Vec::new(),
    );

    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "target data layout has unsupported pointer size or alignment"
    ));
}

#[test]
fn rejects_mixed_target_layouts_across_backend_modules() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let lp64_id = module_ids.allocate();
    let ilp32_id = module_ids.allocate();
    let empty_module = |id, name: &str, target| BackendModule {
        id,
        source_identity: nia_source::SourceIdentity::new(name),
        name: name.to_string(),
        const_eval: BackendConstFacts::default(),
        layouts: BackendLayouts {
            target,
            types: Vec::new(),
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
        trait_object_vtables: Vec::new(),
        generic_instantiations: Vec::new(),
    };
    let program = BackendProgram::new(vec![
        empty_module(lp64_id, "lp64", nia_layout::TargetDataLayout::LP64),
        empty_module(
            ilp32_id,
            "ilp32",
            nia_layout::TargetDataLayout {
                pointer_size: 4,
                pointer_align: 4,
            },
        ),
    ]);

    let output = emit_owned_llvm_ir(program, nia_ty::TypeStore::new());

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "modules disagree on the artifact target data layout"
    ));
}

#[test]
fn rejects_generated_llvm_symbol_collisions_before_emission() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = |index| GlobalDefId {
        module_id,
        def_id: DefId(index),
    };
    let program = BackendProgram::new(vec![BackendModule {
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
        struct_instances: vec![nia_backend_ir::BackendStructInstance {
            def_id: def_id(2),
            name: sym("StructInstance"),
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_type".to_string(),
            fields: Vec::new(),
            is_extern: false,
            span,
        }],
        unions: Vec::new(),
        union_instances: vec![nia_backend_ir::BackendUnionInstance {
            def_id: def_id(3),
            name: sym("UnionInstance"),
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_type".to_string(),
            fields: Vec::new(),
            is_extern: false,
            span,
        }],
        enums: Vec::new(),
        globals: Vec::new(),
        global_instances: vec![nia_backend_ir::BackendGlobalInstance {
            def_id: def_id(1),
            name: sym("global_instance"),
            arg_module_id: module_id,
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_value".to_string(),
            ty: i32_ty,
            is_let: true,
            init: None,
            span,
        }],
        functions: Vec::new(),
        function_instances: vec![BackendFunctionInstance {
            def_id: def_id(0),
            name: sym("function_instance"),
            arg_module_id: module_id,
            self_arg: None,
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_value".to_string(),
            params: Vec::new(),
            return_type: i32_ty,
            is_extern: true,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span,
        }],
        closure_entries: Vec::new(),
        trait_object_vtables: Vec::new(),
        generic_instantiations: Vec::new(),
    }]);

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        "global instance reuses `shared_value` already used by function instance",
        "union instance reuses `shared_type` already used by struct instance",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn rejects_external_collisions_with_compiler_owned_symbols() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = |index| GlobalDefId {
        module_id,
        def_id: DefId(index),
    };
    let module_mangle = nia_mangle::MangleModuleId::from_normalized_source_path("main");
    let owned_global_name = sym("owned_global");
    let owned_function_name = sym("owned_function");
    let owned_global_symbol =
        nia_mangle::mangle_base_symbol_id(def_id(0), module_mangle, owned_global_name);
    let owned_function_symbol =
        nia_mangle::mangle_base_symbol_id(def_id(1), module_mangle, owned_function_name);
    let extern_function = |index, name: &str, link_name: &str| BackendFunction {
        def_id: def_id(index),
        name: sym(name),
        link_name: Some(link_name.to_string()),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: true,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let extern_global = |index, name: &str, link_name: &str| BackendGlobal {
        def_id: def_id(index),
        name: sym(name),
        link_name: Some(link_name.to_string()),
        ty: i32_ty,
        is_let: false,
        is_extern: true,
        init: None,
        span,
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
    let owned_function = BackendFunction {
        def_id: def_id(1),
        name: owned_function_name,
        link_name: None,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(body.clone()),
        span,
    };
    let mut duplicate_definition_a =
        extern_function(10, "duplicate_definition_a", "duplicate_definition");
    duplicate_definition_a.function_body = Some(body.clone());
    let mut duplicate_definition_b =
        extern_function(11, "duplicate_definition_b", "duplicate_definition");
    duplicate_definition_b.function_body = Some(body);
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
            BackendGlobal {
                def_id: def_id(0),
                name: owned_global_name,
                link_name: None,
                ty: i32_ty,
                is_let: false,
                is_extern: false,
                init: Some(StaticInit::Int(0.into())),
                span,
            },
            extern_global(6, "collides_function", &owned_function_symbol),
            extern_global(7, "shared_global", "foreign_shared"),
            extern_global(8, "repeat_global_a", "repeatable_global"),
            extern_global(9, "repeat_global_b", "repeatable_global"),
        ],
        vec![
            owned_function,
            extern_function(2, "collides_global", &owned_global_symbol),
            extern_function(3, "shared_function", "foreign_shared"),
            extern_function(4, "repeat_function_a", "repeatable_function"),
            extern_function(5, "repeat_function_b", "repeatable_function"),
            duplicate_definition_a,
            duplicate_definition_b,
        ],
    );

    drop(interner);
    let output = emit_owned_llvm_ir(program, type_store);

    assert!(output.modules.is_empty());
    for expected in [
        format!("extern function reuses `{owned_global_symbol}` already owned by global"),
        format!("extern global reuses `{owned_function_symbol}` already owned by function"),
        "extern global reuses `foreign_shared` already declared by extern function".to_string(),
        "extern function defines `duplicate_definition` more than once".to_string(),
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, &expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("external symbol collision"))
            .count(),
        4,
        "{:?}",
        output.diagnostics
    );
    assert!(output.diagnostics.iter().all(|diagnostic| {
        !diagnostic.summary.contains("repeatable_function")
            && !diagnostic.summary.contains("repeatable_global")
    }));
}

#[test]
fn rejects_malformed_layout_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let i16_ty = interner.primitive(PrimitiveTy::I16);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let tuple_ty = interner.intern(TyKind::Tuple(vec![i16_ty, i32_ty]));
    let builtin_trait_ty = interner.intern(TyKind::BuiltinTrait {
        trait_id: BuiltinTrait::Sized,
        args: Vec::new(),
    });
    let unpublished_builtin_trait_ty = interner.intern(TyKind::BuiltinTrait {
        trait_id: BuiltinTrait::Iterator,
        args: Vec::new(),
    });
    let generic_ty = interner.intern(TyKind::GenericParam(sym("T")));
    let span = Span::default();
    let enum_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let struct_id = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let generic_struct_id = GlobalDefId {
        module_id,
        def_id: DefId(20),
    };
    let struct_ty = interner.intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let generic_struct_ty = interner.intern(TyKind::Nominal {
        def_id: generic_struct_id,
        args: vec![i16_ty],
        const_args: Vec::new(),
    });
    let instance_key = nia_backend_ir::BackendStructInstanceKey {
        def_id: generic_struct_id,
        args: vec![i16_ty],
        const_args: Vec::new(),
    };
    let instance_layout = StructLayout {
        layout: TypeLayout { size: 4, align: 4 },
        fields: vec![
            FieldLayout {
                def_id: DefId(22),
                offset: 0,
                layout: TypeLayout { size: 4, align: 4 },
            },
            FieldLayout {
                def_id: DefId(21),
                offset: 4,
                layout: TypeLayout { size: 2, align: 2 },
            },
        ],
    };
    let program = BackendProgram {
        modules: vec![BackendModule {
            id: module_id,
            source_identity: nia_source::SourceIdentity::new("main"),
            name: "main".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout {
                    pointer_size: 4,
                    pointer_align: 4,
                },
                types: vec![
                    (usize_ty, TypeLayout { size: 4, align: 4 }),
                    (u8_ty, TypeLayout { size: 1, align: 1 }),
                    (u8_ty, TypeLayout { size: 8, align: 8 }),
                    (i16_ty, TypeLayout { size: 2, align: 2 }),
                    (i32_ty, TypeLayout { size: 4, align: 4 }),
                    (tuple_ty, TypeLayout { size: 4, align: 4 }),
                    (struct_ty, TypeLayout { size: 4, align: 4 }),
                    (generic_struct_ty, TypeLayout { size: 8, align: 4 }),
                    (builtin_trait_ty, TypeLayout { size: 0, align: 1 }),
                ],
                structs: vec![(
                    struct_id,
                    StructLayout {
                        layout: TypeLayout { size: 8, align: 4 },
                        fields: vec![
                            FieldLayout {
                                def_id: DefId(11),
                                offset: 0,
                                layout: TypeLayout { size: 2, align: 2 },
                            },
                            FieldLayout {
                                def_id: DefId(12),
                                offset: 2,
                                layout: TypeLayout { size: 4, align: 4 },
                            },
                        ],
                    },
                )],
                unions: Vec::new(),
                enums: vec![(
                    enum_id,
                    nia_layout::EnumLayout {
                        layout: TypeLayout { size: 8, align: 4 },
                        tag: TypeLayout { size: 1, align: 1 },
                        payload_offset: Some(1),
                        variants: vec![nia_layout::EnumVariantLayout {
                            def_id: DefId(1),
                            payload: TypeLayout { size: 4, align: 4 },
                            fields: vec![
                                nia_layout::EnumFieldLayout {
                                    def_id: Some(DefId(99)),
                                    offset: 1,
                                    layout: TypeLayout { size: 4, align: 4 },
                                },
                                nia_layout::EnumFieldLayout {
                                    def_id: Some(DefId(3)),
                                    offset: 8,
                                    layout: TypeLayout { size: 4, align: 4 },
                                },
                            ],
                        }],
                    },
                )],
                struct_instances: vec![
                    (instance_key.clone(), instance_layout.clone()),
                    (instance_key, instance_layout),
                ],
                union_instances: Vec::new(),
            },
            structs: vec![
                BackendStruct {
                    def_id: struct_id,
                    name: sym("Packet"),
                    generics: Vec::new(),
                    fields: vec![
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(11),
                            },
                            name: sym("small"),
                            ty: i16_ty,
                            span,
                        },
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(12),
                            },
                            name: sym("wide"),
                            ty: i32_ty,
                            span,
                        },
                    ],
                    is_extern: false,
                    span,
                },
                BackendStruct {
                    def_id: generic_struct_id,
                    name: sym("GenericPacket"),
                    generics: vec![sym("T")],
                    fields: vec![
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(21),
                            },
                            name: sym("small"),
                            ty: generic_ty,
                            span,
                        },
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(22),
                            },
                            name: sym("wide"),
                            ty: i32_ty,
                            span,
                        },
                    ],
                    is_extern: false,
                    span,
                },
            ],
            struct_instances: vec![nia_backend_ir::BackendStructInstance {
                def_id: generic_struct_id,
                name: sym("GenericPacketI16"),
                args: vec![i16_ty],
                const_args: Vec::new(),
                symbol: "generic_packet_i16".to_string(),
                fields: vec![
                    BackendField {
                        def_id: GlobalDefId {
                            module_id,
                            def_id: DefId(21),
                        },
                        name: sym("small"),
                        ty: i16_ty,
                        span,
                    },
                    BackendField {
                        def_id: GlobalDefId {
                            module_id,
                            def_id: DefId(22),
                        },
                        name: sym("wide"),
                        ty: i32_ty,
                        span,
                    },
                ],
                is_extern: false,
                span,
            }],
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: vec![BackendEnum {
                def_id: enum_id,
                name: sym("Word"),
                backing_type: usize_ty,
                variants: vec![BackendEnumVariant {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(1),
                    },
                    name: sym("TooLarge"),
                    value: Some(1_i128 << 32),
                    payload: BackendEnumVariantPayload::Named(vec![
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(2),
                            },
                            name: sym("head"),
                            ty: u8_ty,
                            span,
                        },
                        BackendField {
                            def_id: GlobalDefId {
                                module_id,
                                def_id: DefId(3),
                            },
                            name: sym("value"),
                            ty: i32_ty,
                            span,
                        },
                    ]),
                    span,
                }],
                span,
            }],
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
                return_type: unpublished_builtin_trait_ty,
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
                                ty: unpublished_builtin_trait_ty,
                                kind: FunctionExprKind::Integer("0".to_string()),
                            }),
                            span,
                        },
                    }],
                    entry: FunctionBlockId(0),
                    ty: unpublished_builtin_trait_ty,
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "type layout"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "duplicate layout values conflict"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "layout does not match its type and target"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "structural layout does not match its component types and target"
    ));
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("nominal layout does not match its aggregate layout product"))
            .count(),
        2
    );
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "type cannot publish an ABI layout"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "has no ABI layout before LLVM codegen"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "field identity does not match physical declaration order"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "field offset does not match aggregate placement"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "struct instance layout identity is duplicated"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "field extends beyond aggregate storage"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "total layout does not match its declared fields"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "variant discriminant is out of range for its backing type"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "tag layout does not match the backing type"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "payload offset does not match tag and payload alignment"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "payload field identity does not match declaration order"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "payload field layout does not match its declared type"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "payload field offset does not match declaration layout"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "payload field extends beyond variant storage"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "variant payload layout does not match its declared fields"
    ));
}

#[test]
fn validates_aggregate_products_with_structurally_equal_const_args() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
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
    let nominal_ty = interner.intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![signed_arg.clone()],
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
                def_id,
                name: sym("ConstPacket"),
                generics: vec![sym("N")],
                fields: vec![BackendField {
                    def_id: field_id,
                    name: sym("value"),
                    ty: i32_ty,
                    span,
                }],
                is_extern: false,
                span,
            }],
            struct_instances: vec![nia_backend_ir::BackendStructInstance {
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
                is_extern: false,
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
        .into(),
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
fn validates_switch_case_constants_and_uniqueness_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i8_ty = interner.primitive(PrimitiveTy::I8);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i8_ty = interner.primitive(PrimitiveTy::I8);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let char_array_ty = interner.intern(TyKind::Array {
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
    let char_ty = interner.primitive(PrimitiveTy::Char);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let bool_array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: bool_ty,
    });
    let char_array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: char_ty,
    });
    let readonly_bool_array_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_array_ty,
    });
    let readonly_bool_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_ty,
    });
    let mutable_bool_array_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_array_ty,
    });
    let range_exclusive_ty = interner.intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::Exclusive,
        bound: Some(usize_ty),
    });
    let range_from_ty = interner.intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::From,
        bound: Some(usize_ty),
    });
    let range_full_ty = interner.intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::Full,
        bound: None,
    });
    let optional_i32_ty = interner.intern(TyKind::Optional { elem: i32_ty });
    let error_union_ty = interner.intern(TyKind::ErrorUnion {
        error: bool_ty,
        value: i32_ty,
    });
    let global_id = GlobalDefId {
        module_id,
        def_id: DefId(50),
    };
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
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: f32_ty,
            kind: FunctionExprKind::Integer("1".to_string()),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Float("1.0".to_string()),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_array_ty,
            kind: FunctionExprKind::String(vec![1]),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: char_array_ty,
            kind: FunctionExprKind::ByteString(vec![1]),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Char('a' as u32),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::ByteChar("b".to_string()),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Bool(true),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Null,
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::Range(nia_function_ir::FunctionRange {
                start: Some(Box::new(value(usize_ty))),
                end: Some(Box::new(value(usize_ty))),
                inclusive: false,
            }),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: range_exclusive_ty,
            kind: FunctionExprKind::Range(nia_function_ir::FunctionRange {
                start: Some(Box::new(value(bool_ty))),
                end: Some(Box::new(value(usize_ty))),
                inclusive: false,
            }),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: range_full_ty,
            kind: FunctionExprKind::Range(nia_function_ir::FunctionRange {
                start: Some(Box::new(value(usize_ty))),
                end: None,
                inclusive: false,
            }),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: range_from_ty,
            kind: FunctionExprKind::Range(nia_function_ir::FunctionRange {
                start: Some(Box::new(value(usize_ty))),
                end: Some(Box::new(value(usize_ty))),
                inclusive: false,
            }),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::RangeBound {
                range: Box::new(value(range_exclusive_ty)),
                bound: nia_function_ir::FunctionRangeBound::Start,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: usize_ty,
            kind: FunctionExprKind::RangeBound {
                range: Box::new(value(range_from_ty)),
                bound: nia_function_ir::FunctionRangeBound::End,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::BuiltinValue(nia_function_ir::FunctionBuiltinValue::Usize(1)),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::BuiltinValue(nia_function_ir::FunctionBuiltinValue::Layout {
                builtin: nia_ty::LayoutBuiltin::Size,
                ty: i32_ty,
            }),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: usize_ty,
            kind: FunctionExprKind::BuiltinValue(
                nia_function_ir::FunctionBuiltinValue::FieldOffset {
                    ty: i32_ty,
                    field: GlobalDefId {
                        module_id,
                        def_id: DefId(99),
                    },
                },
            ),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: f32_ty,
            kind: FunctionExprKind::BuiltinValue(nia_function_ir::FunctionBuiltinValue::Int(
                nia_ty::IntConst::unsigned(1),
            )),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::BuiltinValue(nia_function_ir::FunctionBuiltinValue::Int(
                nia_ty::IntConst::unsigned(2),
            )),
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: i32_ty,
            kind: FunctionExprKind::StaticArrayPointer {
                allocation: nia_function_ir::PromotedAllocationId::new(module_id, span),
                array: Box::new(value(bool_array_ty)),
                is_readonly: true,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: readonly_bool_ptr_ty,
            kind: FunctionExprKind::StaticArrayPointer {
                allocation: nia_function_ir::PromotedAllocationId::new(module_id, span),
                array: Box::new(value(bool_array_ty)),
                is_readonly: true,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: mutable_bool_array_ptr_ty,
            kind: FunctionExprKind::StaticArrayPointer {
                allocation: nia_function_ir::PromotedAllocationId::new(module_id, span),
                array: Box::new(value(bool_array_ty)),
                is_readonly: true,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: readonly_bool_array_ptr_ty,
            kind: FunctionExprKind::StaticArrayPointer {
                allocation: nia_function_ir::PromotedAllocationId::new(module_id, span),
                array: Box::new(value(bool_ty)),
                is_readonly: true,
            },
        }),
        FunctionOp::Expr(FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::Global(global_id),
        }),
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
        vec![BackendGlobal {
            def_id: global_id,
            name: sym("typed_global"),
            link_name: None,
            ty: i32_ty,
            is_let: true,
            is_extern: false,
            init: Some(StaticInit::Int(0.into())),
            span,
        }],
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
        "integer literal has an invalid type contract",
        "float literal has an invalid type contract",
        "string literal has an invalid type contract",
        "byte string literal has an invalid type contract",
        "char literal has an invalid type contract",
        "byte char literal has an invalid type contract",
        "bool literal has an invalid type contract",
        "null literal has an invalid type contract",
        "expression type is not a range",
        "range bound type does not match its range bound type",
        "full range carries a bound expression",
        "range bound presence does not match its range kind",
        "bound projection result does not match its range bound type",
        "requested bound is not present for the range kind",
        "builtin value has an invalid contract: result type is not usize",
        "field base type is not nominal",
        "integer constant result is not integer-like",
        "integer constant value is outside its result type",
        "static array pointer has an invalid contract: result type is not a pointer",
        "result pointer element does not match the promoted array",
        "readonly metadata does not match its result",
        "promoted value is not an array",
        "global expression type does not match its storage type",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_enum_expression_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let enum_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let unit_variant = GlobalDefId {
        module_id,
        def_id: DefId(1),
    };
    let payload_variant = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let enum_ty = interner.intern(TyKind::Nominal {
        def_id: enum_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let span = Span::default();
    let value = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Null,
    };
    let enum_value = |ty, kind| FunctionOp::Expr(FunctionExpr { span, ty, kind });
    let ops = vec![
        enum_value(
            bool_ty,
            FunctionExprKind::EnumVariant {
                variant: payload_variant,
                fields: vec![value(i32_ty)],
            },
        ),
        enum_value(
            enum_ty,
            FunctionExprKind::EnumVariant {
                variant: payload_variant,
                fields: Vec::new(),
            },
        ),
        enum_value(bool_ty, FunctionExprKind::EnumVariantTag(unit_variant)),
        enum_value(
            bool_ty,
            FunctionExprKind::EnumTag {
                value: Box::new(value(enum_ty)),
            },
        ),
        enum_value(
            bool_ty,
            FunctionExprKind::EnumPayloadField {
                value: Box::new(value(enum_ty)),
                variant: payload_variant,
                field: 0,
            },
        ),
        enum_value(
            i32_ty,
            FunctionExprKind::EnumPayloadField {
                value: Box::new(value(enum_ty)),
                variant: payload_variant,
                field: 1,
            },
        ),
        enum_value(
            i32_ty,
            FunctionExprKind::EnumPayloadField {
                value: Box::new(value(i32_ty)),
                variant: payload_variant,
                field: 0,
            },
        ),
    ];
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(3),
        },
        name: sym("invalid_enum_exprs"),
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
    let enum_layout = nia_layout::EnumLayout {
        layout: TypeLayout { size: 8, align: 4 },
        tag: TypeLayout { size: 4, align: 4 },
        payload_offset: Some(4),
        variants: vec![
            nia_layout::EnumVariantLayout {
                def_id: unit_variant.def_id,
                payload: TypeLayout { size: 0, align: 1 },
                fields: Vec::new(),
            },
            nia_layout::EnumVariantLayout {
                def_id: payload_variant.def_id,
                payload: TypeLayout { size: 4, align: 4 },
                fields: vec![nia_layout::EnumFieldLayout {
                    def_id: None,
                    offset: 0,
                    layout: TypeLayout { size: 4, align: 4 },
                }],
            },
        ],
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
                    (enum_ty, TypeLayout { size: 8, align: 4 }),
                ],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: vec![(enum_id, enum_layout)],
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: vec![BackendEnum {
                def_id: enum_id,
                name: sym("Mode"),
                backing_type: i32_ty,
                variants: vec![
                    BackendEnumVariant {
                        def_id: unit_variant,
                        name: sym("None"),
                        value: Some(0),
                        payload: BackendEnumVariantPayload::Unit,
                        span,
                    },
                    BackendEnumVariant {
                        def_id: payload_variant,
                        name: sym("Some"),
                        value: Some(i128::from(i32::MAX) + 1),
                        payload: BackendEnumVariantPayload::Tuple(vec![i32_ty]),
                        span,
                    },
                ],
                span,
            }],
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![function],
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
    for expected in [
        "variant result type does not match its enum representation",
        "variant value is outside its enum backing type",
        "variant payload field count does not match its declaration",
        "variant tag result does not match the enum backing type",
        "tag result does not match the enum backing type",
        "payload projection result does not match its field type",
        "payload projection field index is out of bounds",
        "payload projection input does not match the variant owner",
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
fn rejects_aggregate_field_with_foreign_owner_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let foreign_module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let point_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let layout_field = GlobalDefId {
        module_id,
        def_id: DefId(2),
    };
    let foreign_field = GlobalDefId {
        module_id: foreign_module_id,
        def_id: layout_field.def_id,
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
                field: foreign_field,
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
                structs: vec![(
                    point_id,
                    StructLayout {
                        layout: TypeLayout { size: 4, align: 4 },
                        fields: vec![FieldLayout {
                            def_id: layout_field.def_id,
                            offset: 0,
                            layout: TypeLayout { size: 4, align: 4 },
                        }],
                    },
                )],
                struct_instances: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: vec![BackendStruct {
                def_id: point_id,
                name: sym("Point"),
                generics: Vec::new(),
                fields: vec![BackendField {
                    def_id: foreign_field,
                    name: sym("x"),
                    ty: i32_ty,
                    span: Span::default(),
                }],
                is_extern: false,
                span: Span::default(),
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
            "does not belong to its aggregate module"
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
    let foreign_store = nia_ty::TypeStore::new();
    let foreign_ty = foreign_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
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
                                        const_args: vec![ConstGenericArg {
                                            ty: foreign_ty,
                                            value: ConstGenericValue::Int(IntConst::unsigned(1)),
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "type belongs to a different compilation session"
    ));
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
        .into(),
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
fn emits_const_only_extern_method_instances_with_c_abi() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
    let receiver_ty = interner.intern(TyKind::Pointer {
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
                link_name: None,
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
                is_extern: false,
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
                is_extern: true,
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

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @const_method_4(ptr)"), "{ir}");
    assert!(ir.contains("call i32 @const_method_4(ptr "), "{ir}");
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
    let wrong_function_params_ty = interner.intern(TyKind::FunctionPointer {
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
    });
    let wrong_function_return_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: bool_ty,
        is_variadic: false,
    });
    let wrong_function_variadic_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: true,
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let unit_ty = interner.intern(TyKind::Tuple(Vec::new()));
    let tuple_ty = interner.intern(TyKind::Tuple(vec![i32_ty]));
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
    let union_ty = interner.intern(TyKind::Nominal {
        def_id: union_id,
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
                    link_name: None,
                    ty: struct_ty,
                    is_let: true,
                    is_extern: false,
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
                    link_name: None,
                    ty: struct_ty,
                    is_let: true,
                    is_extern: false,
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
                    link_name: None,
                    ty: struct_ty,
                    is_let: true,
                    is_extern: false,
                    init: Some(StaticInit::Struct(Vec::new())),
                    span,
                },
                BackendGlobal {
                    def_id: GlobalDefId {
                        module_id,
                        def_id: DefId(6),
                    },
                    name: sym("two_values"),
                    link_name: None,
                    ty: union_ty,
                    is_let: true,
                    is_extern: false,
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

#[test]
fn rejects_enum_variant_with_foreign_owner_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let foreign_module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let foreign_variant = GlobalDefId {
        module_id: foreign_module_id,
        def_id: DefId(1),
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
        .into(),
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
    let malformed_state_pointer_ty = interner.intern(TyKind::Pointer {
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
        symbol: "main__closure_entry__ord__0".to_string(),
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
                is_extern: false,
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
        "closure entry symbol does not match its owner-derived identity"
    ));
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "backend module contains a duplicate closure entry identity"
    ));
}

#[test]
fn validates_closure_entry_call_and_view_contracts_before_llvm() {
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
    let callable_ty = interner.intern(TyKind::Callable {
        is_readonly: true,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let mutable_callable_ty = interner.intern(TyKind::Callable {
        is_readonly: false,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let wrong_callable_ty = interner.intern(TyKind::Callable {
        is_readonly: true,
        params: vec![bool_ty],
        return_type: i32_ty,
    });
    let closure_fn_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let variadic_closure_fn_ty = interner.intern(TyKind::FunctionPointer {
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let wrong_params_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: i32_ty,
        is_variadic: false,
    });
    let wrong_variadic_ty = interner.intern(TyKind::FunctionPointer {
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
            link_name: None,
            ty,
            is_let: true,
            is_extern: false,
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
        link_name: None,
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
        is_extern: true,
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let usize_ty = interner.primitive(PrimitiveTy::Usize);
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
    let declared_arg_ty = interner.intern(TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![signed_arg],
    });
    let referenced_arg_ty = interner.intern(TyKind::Nominal {
        def_id: nominal_def,
        args: Vec::new(),
        const_args: vec![unsigned_arg],
    });
    let function_id = GlobalDefId {
        module_id,
        def_id: DefId(0),
    };
    let pointer_ty = interner.intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: i32_ty,
        is_variadic: false,
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
                link_name: None,
                ty: pointer_ty,
                is_let: true,
                is_extern: false,
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
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "function address parameter types do not match its target"
    ));
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
fn validates_static_global_address_pointee_and_mutability_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let readonly_bool_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_ty,
    });
    let mutable_i32_ptr_ty = interner.intern(TyKind::Pointer {
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
        link_name: None,
        ty,
        is_let: true,
        is_extern: false,
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
            link_name: None,
            ty: i32_ty,
            is_let: true,
            is_extern: false,
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
fn validates_backend_ir_place_type_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let i32_array_ty = interner.intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: i32_ty,
    });
    let bool_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_ty,
    });
    let i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.intern(TyKind::Pointer {
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

#[test]
fn validates_backend_ir_assignment_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let f32_ty = interner.primitive(PrimitiveTy::F32);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let u8_ty = interner.primitive(PrimitiveTy::U8);
    let unit_ty = interner.intern(TyKind::Tuple(Vec::new()));
    let mutable_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let local_place = |local_id, ty| FunctionPlace {
        span,
        ty,
        base: FunctionPlaceBase::Local(local_id),
        elems: Vec::new(),
    };
    let integer = |ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Integer("1".to_string()),
    };
    let assignment = |ty, place, op, rhs| {
        FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Assign {
                place,
                op,
                rhs: Box::new(rhs),
            },
        })
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
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("integer"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: i32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("immutable"),
                    kind: FunctionLocalKind::ImmutableBinding,
                    ty: i32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(2),
                    name: local_name("float"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: f32_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(3),
                    name: local_name("pointer"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: mutable_i32_ptr_ty,
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
                    assignment(
                        i32_ty,
                        local_place(LocalId(0), i32_ty),
                        AssignOp::Assign,
                        integer(i32_ty),
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(0), i32_ty),
                        AssignOp::Assign,
                        FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(0), i32_ty),
                        AssignOp::Add,
                        FunctionExpr {
                            span,
                            ty: bool_ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(2), f32_ty),
                        AssignOp::BitAnd,
                        FunctionExpr {
                            span,
                            ty: f32_ty,
                            kind: FunctionExprKind::Float("1.0".to_string()),
                        },
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(1), i32_ty),
                        AssignOp::Assign,
                        integer(i32_ty),
                    ),
                    assignment(
                        unit_ty,
                        FunctionPlace {
                            span,
                            ty: i32_ty,
                            base: FunctionPlaceBase::Deref(Box::new(FunctionExpr {
                                span,
                                ty: readonly_i32_ptr_ty,
                                kind: FunctionExprKind::Null,
                            })),
                            elems: Vec::new(),
                        },
                        AssignOp::Assign,
                        integer(i32_ty),
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(3), readonly_i32_ptr_ty),
                        AssignOp::Assign,
                        FunctionExpr {
                            span,
                            ty: readonly_i32_ptr_ty,
                            kind: FunctionExprKind::Null,
                        },
                    ),
                    assignment(
                        unit_ty,
                        local_place(LocalId(0), i32_ty),
                        AssignOp::Shl,
                        integer(u8_ty),
                    ),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Discard(Box::new(integer(i32_ty))),
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Try {
                            expr: Box::new(integer(i32_ty)),
                        },
                    }),
                ],
                terminator: FunctionTerminator::Tail {
                    value: Some(integer(i32_ty)),
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
                (u8_ty, TypeLayout { size: 1, align: 1 }),
                (unit_ty, TypeLayout { size: 0, align: 1 }),
                (mutable_i32_ptr_ty, TypeLayout { size: 8, align: 8 }),
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
        "result type is not unit",
        "right-hand side type does not match the target",
        "binary operands do not have a compatible type",
        "binary operand type is not supported by the operation",
        "target storage is not writable",
        "target type is only a readonly storage view",
        "discard result type is not unit",
        "propagation expression was not lowered to a CFG terminator",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn validates_backend_ir_trait_object_expression_contracts_before_llvm() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.primitive(PrimitiveTy::Bool);
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let source_trait = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let target_trait = GlobalDefId {
        module_id,
        def_id: DefId(11),
    };
    let object = |trait_id, is_readonly| {
        interner.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Source(trait_id),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            associated_type_bindings: Vec::new(),
        })
    };
    let source_object_ty = object(source_trait, true);
    let target_object_ty = object(target_trait, true);
    let mutable_target_object_ty = object(target_trait, false);
    let readonly_i32_ptr_ty = interner.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let span = Span::default();
    let local = |id, ty| FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Local(LocalId(id)),
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
            locals: vec![
                FunctionLocal {
                    id: LocalId(0),
                    name: local_name("object"),
                    kind: FunctionLocalKind::MutableBinding,
                    ty: source_object_ty,
                    span,
                },
                FunctionLocal {
                    id: LocalId(1),
                    name: local_name("pointer"),
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
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::TraitObjectUpcast {
                            expr: Box::new(local(0, source_object_ty)),
                            source_ty: source_object_ty,
                            target_ty: target_object_ty,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: mutable_target_object_ty,
                        kind: FunctionExprKind::TraitObjectUpcast {
                            expr: Box::new(local(0, source_object_ty)),
                            source_ty: source_object_ty,
                            target_ty: mutable_target_object_ty,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: mutable_target_object_ty,
                        kind: FunctionExprKind::TraitObjectCoercion {
                            expr: Box::new(local(1, readonly_i32_ptr_ty)),
                            target_ty: mutable_target_object_ty,
                            self_ty: bool_ty,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::TraitObjectCoercion {
                            expr: Box::new(local(1, readonly_i32_ptr_ty)),
                            target_ty: i32_ty,
                            self_ty: i32_ty,
                        },
                    }),
                    FunctionOp::Expr(FunctionExpr {
                        span,
                        ty: target_object_ty,
                        kind: FunctionExprKind::TraitObjectCoercion {
                            expr: Box::new(local(1, readonly_i32_ptr_ty)),
                            target_ty: target_object_ty,
                            self_ty: i32_ty,
                        },
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
                (i32_ty, TypeLayout { size: 4, align: 4 }),
                (source_object_ty, TypeLayout { size: 16, align: 8 }),
                (target_object_ty, TypeLayout { size: 16, align: 8 }),
                (mutable_target_object_ty, TypeLayout { size: 16, align: 8 }),
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
        "upcast result type does not match target metadata",
        "upcast cannot strengthen readonly access",
        "coercion self type does not match source element",
        "coercion target is not a trait object",
        "coercion cannot strengthen readonly access",
        "coercion target vtable is missing",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
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
                                const_args: Vec::new(),
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
