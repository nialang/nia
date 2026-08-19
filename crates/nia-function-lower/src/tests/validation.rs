// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn validates_lowered_function_body() {
    let ty = test_ty();
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span: Span::default(),
            kind: TypedStmtKind::Expr(TypedExpr {
                span: Span::default(),
                ty,
                kind: TypedExprKind::Block(TypedBody {
                    span: Span::default(),
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span: Span::default(),
                        kind: TypedStmtKind::Defer(int_expr(1)),
                    }],
                    tail: None,
                    ty,
                }),
            }),
        }],
        tail: Some(Box::new(int_expr(0))),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    validate_function_body(&function_body).expect("valid function body");
}

#[test]
fn validates_defer_body_references_to_enclosing_locals() {
    let span = Span::default();
    let ty = test_ty();
    let local = TypedLocal {
        id: LocalId(1),
        name: local_name("value"),
        kind: TypedLocalKind::MutableBinding,
        ty,
        span,
    };
    let body = TypedBody {
        span,
        locals: vec![local],
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Defer(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Local(LocalId(1)),
            }),
        }],
        tail: Some(Box::new(int_expr(0))),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    validate_function_body(&function_body).expect("valid defer local capture");
}

#[test]
fn trap_tail_lowers_as_effect_only_even_with_value_type() {
    let ty = test_ty();
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Trap,
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert!(
        function_body.blocks.iter().any(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op,
                    FunctionOp::Expr(FunctionExpr {
                        kind: FunctionExprKind::Trap,
                        ..
                    })
                )
            })
        }),
        "{function_body:#?}"
    );
    assert!(
        function_body
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::Error { .. })),
        "{function_body:#?}"
    );
    validate_function_body(&function_body).expect("trap tail should be valid effect IR");
}

#[test]
fn rejects_error_expr_before_function_ir_is_built() {
    let ty = test_ty();
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Error,
        })),
        ty,
    };

    let error = lower_test_function_body(&body).expect_err("error expr must not lower");

    assert!(
        error
            .message
            .contains("error expression escaped into function lowering input"),
        "{error:?}"
    );
}

#[test]
fn rejects_error_expr_nested_in_for_pattern_before_function_ir_is_built() {
    let ty = test_ty();
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span: Span::default(),
            kind: TypedStmtKind::ForIn(Box::new(TypedForIn {
                pattern: TypedPattern {
                    ty,
                    span: Span::default(),
                    kind: TypedPatternKind::Expr(TypedExpr {
                        span: Span::default(),
                        ty,
                        kind: TypedExprKind::Error,
                    }),
                },
                item_ty: ty,
                bool_ty: ty,
                iterable_self_ty: ty,
                iterator_ty: ty,
                iter: int_expr(0),
                body: empty_body(ty),
            })),
        }],
        tail: None,
        ty,
    };

    let error = lower_test_function_body(&body).expect_err("error pattern must not lower");

    assert!(
        error
            .message
            .contains("error expression escaped into function lowering input"),
        "{error:?}"
    );
}

#[test]
fn rejects_error_place_before_function_ir_is_built() {
    let ty = test_ty();
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty,
                    base: PlaceBase::Error,
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(int_expr(1)),
            },
        })),
        ty,
    };

    let error = lower_test_function_body(&body).expect_err("error place must not lower");

    assert!(
        error
            .message
            .contains("error place escaped into function lowering input"),
        "{error:?}"
    );
}

#[test]
fn rejects_error_exprs_hidden_in_lowering_operand_containers() {
    let ty = test_ty();
    let error = || TypedExpr {
        span: Span::default(),
        ty,
        kind: TypedExprKind::Error,
    };
    let place = || TypedPlace {
        span: Span::default(),
        ty,
        base: PlaceBase::Deref(Box::new(error())),
        elems: Vec::new(),
    };
    let module_id = ModuleIdAllocator::new().allocate();
    let malformed = vec![
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Assign {
                place: place(),
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(int_expr(0)),
            },
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Call {
                callee: nia_body_ir::TypedCallee::FunctionPointer(Box::new(error())),
                args: Vec::new(),
            },
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::InlineAsm(nia_body_ir::TypedInlineAsm {
                code: String::new(),
                inputs: vec![nia_body_ir::TypedAsmInput {
                    constraint: String::new(),
                    value: error(),
                    span: Span::default(),
                }],
                outputs: Vec::new(),
                clobbers: Vec::new(),
                options: Vec::new(),
            }),
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::InlineAsm(nia_body_ir::TypedInlineAsm {
                code: String::new(),
                inputs: Vec::new(),
                outputs: vec![nia_body_ir::TypedAsmOutput {
                    constraint: String::new(),
                    place: place(),
                    span: Span::default(),
                }],
                clobbers: Vec::new(),
                options: Vec::new(),
            }),
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::UnionStorageLiteral {
                bytes: Vec::new(),
                relocations: vec![nia_body_ir::TypedUnionRelocation {
                    offset: 0,
                    width: 0,
                    allocation: nia_body_ir::PromotedAllocationId::new(module_id, Span::default()),
                    pointee: Box::new(error()),
                }],
            },
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                op: MemoryIntrinsicOp::Copy,
                elem_ty: ty,
                dest: Box::new(error()),
                source: TypedMemoryIntrinsicSource::Slice(Box::new(int_expr(0))),
            }),
        },
        TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Atomic(nia_body_ir::TypedAtomic::Load {
                ty,
                ptr: Box::new(error()),
                order: AtomicOrder::Acquire,
            }),
        },
    ];

    for expr in malformed {
        let body = TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Expr(expr),
            }],
            tail: None,
            ty,
        };

        let error = lower_test_function_body(&body)
            .expect_err("hidden error expression must not reach function IR");
        assert!(
            error
                .message
                .contains("error expression escaped into function lowering input"),
            "{error:?}"
        );
    }
}

#[test]
fn rejects_memory_intrinsic_in_value_position_before_function_ir_is_built() {
    let ty = test_ty();
    let memory = TypedExpr {
        span: Span::default(),
        ty,
        kind: TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
            op: MemoryIntrinsicOp::Copy,
            elem_ty: ty,
            dest: Box::new(int_expr(0)),
            source: TypedMemoryIntrinsicSource::Slice(Box::new(int_expr(1))),
        }),
    };
    let body = TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::OptionalSome {
                expr: Box::new(memory),
            },
        })),
        ty,
    };

    let error = lower_test_function_body(&body).expect_err("memory intrinsic must not be a value");

    assert!(
        error
            .message
            .contains("memory intrinsic expression used where a value is required"),
        "{error:?}"
    );
}

#[test]
fn rejects_atomic_store_and_fence_in_value_position_before_function_ir_is_built() {
    let ty = test_ty();
    for atomic in [
        nia_body_ir::TypedAtomic::Store {
            ty,
            ptr: Box::new(int_expr(0)),
            value: Box::new(int_expr(1)),
            order: AtomicOrder::Monotonic,
        },
        nia_body_ir::TypedAtomic::Fence {
            order: AtomicOrder::SeqCst,
        },
    ] {
        let body = TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: Vec::new(),
            tail: Some(Box::new(TypedExpr {
                span: Span::default(),
                ty,
                kind: TypedExprKind::OptionalSome {
                    expr: Box::new(TypedExpr {
                        span: Span::default(),
                        ty,
                        kind: TypedExprKind::Atomic(atomic),
                    }),
                },
            })),
            ty,
        };

        let error =
            lower_test_function_body(&body).expect_err("effect-only atomic must not be a value");

        assert!(
            error
                .message
                .contains("expression used where a value is required"),
            "{error:?}"
        );
    }
}

#[test]
fn rejects_missing_successor_block() {
    let mut function_body = manual_function_body_for_scope_edges();
    function_body.blocks[0].terminator = FunctionTerminator::Branch {
        target: FunctionBlockId(99),
        span: Span::default(),
    };

    let error = validate_function_body(&function_body).expect_err("invalid successor");

    assert!(error.message.contains("missing block `99`"), "{error:?}");
}

#[test]
fn rejects_missing_block_scope() {
    let mut function_body = manual_function_body_for_scope_edges();
    function_body.blocks[0].scope = FunctionScopeId(99);

    let error = validate_function_body(&function_body).expect_err("invalid scope");

    assert!(error.message.contains("missing scope `99`"), "{error:?}");
}

#[test]
fn rejects_missing_local_reference() {
    let ty = test_ty();
    let function_body = FunctionBody {
        span: Span::default(),
        locals: Vec::new(),
        scopes: vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span: Span::default(),
            ops: Vec::new(),
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span: Span::default(),
                    ty,
                    kind: FunctionExprKind::Local(LocalId(9)),
                }),
                span: Span::default(),
            },
        }],
        entry: FunctionBlockId(0),
        ty,
    };

    let error = validate_function_body(&function_body).expect_err("invalid local");

    assert!(error.message.contains("missing local `9`"), "{error:?}");
}

#[test]
fn rejects_invalid_defer_body_recursively() {
    let ty = test_ty();
    let function_body = FunctionBody {
        span: Span::default(),
        locals: Vec::new(),
        scopes: vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span: Span::default(),
            ops: vec![FunctionOp::Defer(FunctionDeferBody {
                span: Span::default(),
                scopes: vec![FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span: Span::default(),
                }],
                blocks: Vec::new(),
                entry: FunctionBlockId(99),
            })],
            terminator: FunctionTerminator::Tail {
                value: None,
                span: Span::default(),
            },
        }],
        entry: FunctionBlockId(0),
        ty,
    };

    let error = validate_function_body(&function_body).expect_err("invalid defer");

    assert!(error.message.contains("missing block `99`"), "{error:?}");
}
