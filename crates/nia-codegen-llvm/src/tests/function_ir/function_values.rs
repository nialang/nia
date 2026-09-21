use super::*;

#[test]
fn validates_backend_ir_assignment_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let unit_ty = interner.test_intern(TyKind::Tuple(Vec::new()));
    let mutable_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let source_trait = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let target_trait = GlobalDefId {
        module_id,
        def_id: DefId(11),
    };
    let object = |trait_id, is_readonly| {
        interner.test_intern(TyKind::TraitObject {
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
    let readonly_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
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
fn validates_backend_ir_unresolved_builtin_trait_method_call_before_llvm() {
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
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty: i32_ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::BuiltinTraitMethodCall {
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
