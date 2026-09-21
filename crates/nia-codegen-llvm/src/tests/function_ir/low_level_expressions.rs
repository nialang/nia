use super::*;


#[test]
fn validates_atomic_contracts_before_llvm() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let unit_ty = interner.test_intern(TyKind::Tuple(Vec::new()));
    let optional_i32_ty = interner.test_intern(TyKind::Optional { elem: i32_ty });
    let readonly_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let mutable_i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let mutable_bool_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_ty,
    });
    let mutable_ptr_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: mutable_i32_ptr_ty,
    });
    let readonly_f32_ptr_ty = interner.test_intern(TyKind::Pointer {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let mutable_i32_slice_ty = interner.test_intern(TyKind::Slice {
        is_readonly: false,
        elem: i32_ty,
    });
    let readonly_i32_slice_ty = interner.test_intern(TyKind::Slice {
        is_readonly: true,
        elem: i32_ty,
    });
    let readonly_bool_slice_ty = interner.test_intern(TyKind::Slice {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let u32_ty = interner.test_primitive(PrimitiveTy::U32);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let optional_char_ty = interner.test_intern(TyKind::Optional { elem: char_ty });
    let byte_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: u8_ty,
    });
    let i32_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let i32x4_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let boolx4_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::Bool,
        lanes: 4,
    });
    let boolx65_ty = interner.test_intern(TyKind::Vector {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let vector_i32_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let pointer_i32_ty = interner.test_intern(TyKind::Pointer {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let i64_ty = interner.test_primitive(PrimitiveTy::I64);
    let u32_ty = interner.test_primitive(PrimitiveTy::U32);
    let vector_i32x4_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 4,
    });
    let vector_i32x8_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I32,
        lanes: 8,
    });
    let vector_i64x4_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I64,
        lanes: 4,
    });
    let vector_i64x8_ty = interner.test_intern(TyKind::Vector {
        elem: PrimitiveTy::I64,
        lanes: 8,
    });
    let pointer_i32_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: i32_ty,
    });
    let volatile_pointer_i32_ty = interner.test_intern(TyKind::VolatilePointer {
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
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = nia_ty::TypeStore::new().expect("create type store");
    let interner = type_store.append_for_module(module_id);
    let bool_ty = interner.test_primitive(PrimitiveTy::Bool);
    let char_ty = interner.test_primitive(PrimitiveTy::Char);
    let f32_ty = interner.test_primitive(PrimitiveTy::F32);
    let i32_ty = interner.test_primitive(PrimitiveTy::I32);
    let usize_ty = interner.test_primitive(PrimitiveTy::Usize);
    let u8_ty = interner.test_primitive(PrimitiveTy::U8);
    let bool_array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: bool_ty,
    });
    let char_array_ty = interner.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstValue(1),
        elem: char_ty,
    });
    let readonly_bool_array_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_array_ty,
    });
    let readonly_bool_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: bool_ty,
    });
    let mutable_bool_array_ptr_ty = interner.test_intern(TyKind::Pointer {
        is_readonly: false,
        elem: bool_array_ty,
    });
    let range_exclusive_ty = interner.test_intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::Exclusive,
        bound: Some(usize_ty),
    });
    let range_from_ty = interner.test_intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::From,
        bound: Some(usize_ty),
    });
    let range_full_ty = interner.test_intern(TyKind::Range {
        kind: nia_ty::RangeTyKind::Full,
        bound: None,
    });
    let optional_i32_ty = interner.test_intern(TyKind::Optional { elem: i32_ty });
    let error_union_ty = interner.test_intern(TyKind::ErrorUnion {
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
            linkage: BackendLinkage::Nia,
            ty: i32_ty,
            is_let: true,
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
