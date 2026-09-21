use super::*;


#[test]
fn validates_enum_expression_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
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
    let enum_ty = interner.test_intern(TyKind::Nominal {
        def_id: enum_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let unit_constructor_ty = interner.test_intern(TyKind::FunctionPointer {
        params: Vec::new(),
        return_type: enum_ty,
        is_variadic: false,
    });
    let wrong_param_constructor_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![bool_ty],
        return_type: enum_ty,
        is_variadic: false,
    });
    let wrong_return_constructor_ty = interner.test_intern(TyKind::FunctionPointer {
        params: vec![i32_ty],
        return_type: bool_ty,
        is_variadic: false,
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
            unit_constructor_ty,
            FunctionExprKind::EnumConstructor(unit_variant),
        ),
        enum_value(bool_ty, FunctionExprKind::EnumConstructor(payload_variant)),
        enum_value(
            wrong_param_constructor_ty,
            FunctionExprKind::EnumConstructor(payload_variant),
        ),
        enum_value(
            wrong_return_constructor_ty,
            FunctionExprKind::EnumConstructor(payload_variant),
        ),
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
            symbol_package_identity: "test/package@0".into(),
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
        .try_into()
        .expect("build backend modules"),
    };
    drop(interner);

    let output = emit_owned_llvm_ir(program, type_store);
    assert!(output.modules.is_empty());
    for expected in [
        "unit variant cannot be used as a constructor function",
        "constructor value is not a function pointer",
        "constructor function parameters do not match the variant payload",
        "constructor function return type does not match the variant owner",
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
