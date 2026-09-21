use super::*;


#[test]
fn rejects_ordinary_definition_with_foreign_module_owner_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let foreign_module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id: foreign_module_id,
            def_id: DefId(0),
        },
        name: sym("foreign_owner"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(2),
        elem: u8_ty,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("oversized"),
        linkage: BackendLinkage::Nia,
        ty: array_ty,
        is_let: false,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let vector_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(2),
        elem: vector_ty,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("vectors"),
        linkage: BackendLinkage::Nia,
        ty: array_ty,
        is_let: true,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let vector_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::U8,
        lanes: 2,
    });
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_vector"),
        linkage: BackendLinkage::Nia,
        ty: vector_ty,
        is_let: true,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let tuple_ty = interner.test_intern(TyKind::Tuple(vec![i32_ty, bool_ty]));
    let global = BackendGlobal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_tuple"),
        linkage: BackendLinkage::Nia,
        ty: tuple_ty,
        is_let: true,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let vector_i32_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let char_array_ty = interner.test_intern(TyKind::Array {
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
        linkage: BackendLinkage::Nia,
        ty,
        is_let: true,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let pointer_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: u8_ty,
    });
    let array_ty = interner.test_intern(TyKind::Array {
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
        linkage: BackendLinkage::Nia,
        ty: array_ty,
        is_let: false,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("word"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let span = Span::default();
    let function = BackendFunction {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
        name: sym("invalid_usize"),
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let lp64_id = module_ids.allocate().expect("allocate module ID");
    let ilp32_id = module_ids.allocate().expect("allocate module ID");
    let empty_module = |id, name: &str, target| BackendModule {
        id,
        source_identity: nia_source::SourceIdentity::new(name),
        symbol_package_identity: "test/package@0".into(),
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
    ])
    .expect("build backend program");

    let output = emit_owned_llvm_ir(
        program,
        nia_ty::TypeStore::new().expect("create type store"),
    );

    assert!(output.modules.is_empty());
    assert!(has_internal_diagnostic(
        &output.diagnostics,
        codes::INVALID_BACKEND_IR,
        "modules disagree on the artifact target data layout"
    ));
}

#[test]
fn rejects_generated_llvm_symbol_collisions_before_emission() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = |index| GlobalDefId {
        module_id,
        def_id: DefId(index),
    };
    let program = BackendProgram::new(vec![BackendModule {
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
        struct_instances: vec![nia_backend_ir::BackendStructInstance {
            is_extern: false,
            def_id: def_id(2),
            name: sym("StructInstance"),
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_type".to_string(),
            fields: Vec::new(),
            span,
        }],
        unions: Vec::new(),
        union_instances: vec![nia_backend_ir::BackendUnionInstance {
            is_extern: false,
            def_id: def_id(3),
            name: sym("UnionInstance"),
            args: Vec::new(),
            const_args: Vec::new(),
            symbol: "shared_type".to_string(),
            fields: Vec::new(),
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
    }])
    .expect("build backend program");

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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let span = Span::default();
    let def_id = |index| GlobalDefId {
        module_id,
        def_id: DefId(index),
    };
    let module_mangle = nia_mangle::MangleModuleId::from_normalized_source_path("main");
    let owned_global_name = sym("owned_global");
    let owned_function_name = sym("owned_function");
    let owned_global_symbol = nia_mangle::mangle_definition_symbol_canonical(
        "test/package@0",
        def_id(0),
        module_mangle,
        nia_mangle::mangle_symbol_id(owned_global_name),
        MangleSymbolKind::Global,
    )
    .expect("mangle owned global symbol");
    let owned_function_symbol = nia_mangle::mangle_definition_symbol_canonical(
        "test/package@0",
        def_id(1),
        module_mangle,
        nia_mangle::mangle_symbol_id(owned_function_name),
        MangleSymbolKind::Function,
    )
    .expect("mangle owned function symbol");
    let extern_function = |index, name: &str, external_symbol: &str| BackendFunction {
        def_id: def_id(index),
        name: sym(name),
        linkage: BackendLinkage::ExternImport {
            symbol: external_symbol.to_string(),
        },
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: None,
        span,
    };
    let extern_global = |index, name: &str, external_symbol: &str| BackendGlobal {
        def_id: def_id(index),
        name: sym(name),
        linkage: BackendLinkage::ExternImport {
            symbol: external_symbol.to_string(),
        },
        ty: i32_ty,
        is_let: false,
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
        linkage: BackendLinkage::Nia,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: i32_ty,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: Default::default(),
        function_body: Some(body.clone()),
        span,
    };
    let mut duplicate_definition_a =
        extern_function(10, "duplicate_definition_a", "duplicate_definition");
    duplicate_definition_a.function_body = Some(body.clone());
    duplicate_definition_a.linkage = BackendLinkage::ExternExport {
        symbol: "duplicate_definition".into(),
    };
    let mut duplicate_definition_b =
        extern_function(11, "duplicate_definition_b", "duplicate_definition");
    duplicate_definition_b.function_body = Some(body);
    duplicate_definition_b.linkage = BackendLinkage::ExternExport {
        symbol: "duplicate_definition".into(),
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
            BackendGlobal {
                def_id: def_id(0),
                name: owned_global_name,
                linkage: BackendLinkage::Nia,
                ty: i32_ty,
                is_let: false,
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let i16_ty = interner.test_primitive(PrimitiveTy::I16);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let tuple_ty = interner.test_intern(TyKind::Tuple(vec![i16_ty, i32_ty]));
    let builtin_trait_ty = interner.test_intern(TyKind::BuiltinTrait {
        trait_id: BuiltinTrait::Sized,
        args: Vec::new(),
    });
    let unpublished_builtin_trait_ty = interner.test_intern(TyKind::BuiltinTrait {
        trait_id: BuiltinTrait::Iterator,
        args: Vec::new(),
    });
    let generic_ty = interner.test_intern(TyKind::GenericParam(sym("T")));
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
    let struct_ty = interner.test_intern(TyKind::Nominal {
        def_id: struct_id,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let generic_struct_ty = interner.test_intern(TyKind::Nominal {
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
            symbol_package_identity: "test/package@0".into(),
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
                    is_extern: false,
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
                    span,
                },
                BackendStruct {
                    is_extern: false,
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
                    span,
                },
            ],
            struct_instances: vec![nia_backend_ir::BackendStructInstance {
                is_extern: false,
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
                linkage: BackendLinkage::Nia,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: unpublished_builtin_trait_ty,
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
        .try_into()
        .expect("build backend modules"),
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
