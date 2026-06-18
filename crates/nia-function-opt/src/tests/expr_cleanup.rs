use super::*;

#[test]
fn removes_same_type_cast_wrappers_recursively() {
    let span = Span::default();
    let ty = test_ty();
    let mut expr = FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Cast {
                expr: Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                ty,
            },
        })),
    };

    simplify_same_type_casts_in_expr(&mut expr);

    let FunctionExprKind::Discard(inner) = expr.kind else {
        panic!("expected discard wrapper");
    };
    assert!(matches!(inner.kind, FunctionExprKind::Local(LocalId(0))));
}

#[test]
fn preserves_casts_that_change_type() {
    let span = Span::default();
    let source_ty = test_ty();
    let target_ty = nia_ids::InternedTyId::new(
        nia_ids::ModuleId(0),
        nia_ids::TyInternerIndex::from_interner_index(1),
    );
    let mut expr = FunctionExpr {
        span,
        ty: target_ty,
        kind: FunctionExprKind::Cast {
            expr: Box::new(FunctionExpr {
                span,
                ty: source_ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            ty: target_ty,
        },
    };

    simplify_same_type_casts_in_expr(&mut expr);

    assert!(matches!(expr.kind, FunctionExprKind::Cast { .. }));
}

#[test]
fn simplifies_constant_logical_exprs_without_dropping_effects() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: Vec::new(),
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Binary {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(true),
                    }),
                    op: nia_ast::BinaryOp::And,
                    rhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Binary {
                            lhs: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Local(LocalId(0)),
                            }),
                            op: nia_ast::BinaryOp::Or,
                            rhs: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Bool(false),
                            }),
                        },
                    }),
                },
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "flag".to_string(),
        kind: FunctionLocalKind::Param,
        ty,
        span,
    }];

    assert!(simplify_constant_logical_exprs(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
    validate_function_body(&body).expect("logical-simplified body should remain valid");
}

#[test]
fn preserves_constant_logical_rhs_when_lhs_must_be_evaluated() {
    let span = Span::default();
    let ty = test_ty();
    let mut expr = FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Binary {
            lhs: Box::new(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            op: nia_ast::BinaryOp::And,
            rhs: Box::new(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Bool(false),
            }),
        },
    };

    assert!(!simplify_constant_logical_expr(&mut expr));
    assert!(matches!(
        expr.kind,
        FunctionExprKind::Binary {
            op: nia_ast::BinaryOp::And,
            ..
        }
    ));
}

#[test]
fn removes_noop_local_store_ops() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                span,
            },
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(1)),
                },
                span,
            },
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_noop_local_stores(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                kind: FunctionExprKind::Local(LocalId(1)),
                ..
            },
            ..
        }
    ));
}

#[test]
fn removes_noop_local_store_after_same_type_cast_simplification() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Cast {
                    expr: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    ty,
                },
            },
            span,
        }],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    simplify_same_type_casts_in_blocks(&mut body.blocks);
    remove_noop_local_stores(&mut body.blocks);

    assert!(body.blocks[0].ops.is_empty());
}

#[test]
fn removes_pure_expr_ops_but_preserves_effectful_expr_ops() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                })),
            }),
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                        module_id: nia_ids::ModuleId(0),
                        def_id: nia_ids::DefId(0),
                    }),
                    args: Vec::new(),
                },
            }),
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Call { .. },
            ..
        })
    ));
}

#[test]
fn removes_pure_wrapper_expr_ops() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Binary {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    op: nia_ast::BinaryOp::Add,
                    rhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                },
            }),
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::ArrayLiteral {
                    elems: FunctionArrayElements::List(vec![FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Cast {
                            expr: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Bool(false),
                            }),
                            ty,
                        },
                    }]),
                },
            }),
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert!(body.blocks[0].ops.is_empty());
}

#[test]
fn preserves_aggregate_expr_ops_with_effectful_elements() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::ArrayLiteral {
                elems: FunctionArrayElements::List(vec![FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                }]),
            },
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::ArrayLiteral { .. },
            ..
        })
    ));
}
