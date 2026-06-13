// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn lowers_statement_switch_into_switch_terminator() {
    let ty = test_ty();
    let body = switch_stmt_body(vec![
        switch_expr_arm(1, TypedSwitchArmBody::Expr(int_expr(10))),
        switch_default_arm(TypedSwitchArmBody::Expr(int_expr(20))),
    ]);

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Switch {
        arms,
        default,
        fallback,
        ..
    } = &function_body.blocks[0].terminator
    else {
        panic!("expected switch terminator");
    };

    assert_eq!(arms.len(), 1);
    assert_eq!(arms[0].target, FunctionBlockId(2));
    assert_eq!(*default, Some(FunctionBlockId(3)));
    assert_eq!(*fallback, FunctionBlockId(1));
    assert_eq!(
        function_body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(2), FunctionBlockId(3)]
    );
    assert_eq!(
        function_body
            .block(arms[0].target)
            .expect("case block")
            .scope,
        FunctionScopeId(1)
    );
    assert_eq!(
        function_body
            .block(default.unwrap())
            .expect("default block")
            .scope,
        FunctionScopeId(2)
    );
    assert_eq!(
        function_body.block(*fallback).expect("merge block").scope,
        FunctionScopeId(0)
    );
    assert_eq!(body.ty, ty);
}

#[test]
fn statement_switch_without_default_falls_back_to_merge() {
    let body = switch_stmt_body(vec![switch_expr_arm(
        1,
        TypedSwitchArmBody::Expr(int_expr(10)),
    )]);

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Switch {
        default, fallback, ..
    } = function_body.blocks[0].terminator
    else {
        panic!("expected switch terminator");
    };

    assert_eq!(default, None);
    assert_eq!(
        function_body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(2), fallback]
    );
    assert_eq!(
        function_body.block(fallback).expect("merge block").scope,
        FunctionScopeId(0)
    );
}

#[test]
fn statement_switch_with_range_patterns_lowers_to_condition_chain() {
    let body = switch_stmt_body(vec![
        switch_range_arm(0, 3, false, TypedSwitchArmBody::Expr(int_expr(10))),
        switch_expr_arm(7, TypedSwitchArmBody::Expr(int_expr(20))),
        switch_default_arm(TypedSwitchArmBody::Expr(int_expr(30))),
    ]);

    let function_body = lower_function_body(&body);

    assert!(
        function_body
            .block(function_body.entry)
            .expect("entry block")
            .ops
            .iter()
            .any(|op| matches!(op, FunctionOp::StoreLocal { .. })),
        "{function_body:#?}"
    );
    assert!(
        function_body
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::If { .. })),
        "{function_body:#?}"
    );
    assert!(
        !function_body
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::Switch { .. })),
        "{function_body:#?}"
    );
}

#[test]
fn statement_if_pattern_binding_stores_tagged_union_payload() {
    let ty = test_ty();
    let target_local = LocalId(0);
    let payload_local = LocalId(1);
    let span = Span::default();
    let body = TypedBody {
        span,
        locals: vec![
            TypedLocal {
                id: target_local,
                name: "value".to_string(),
                kind: TypedLocalKind::Param,
                ty,
                span,
            },
            TypedLocal {
                id: payload_local,
                name: "x".to_string(),
                kind: TypedLocalKind::Binding,
                ty,
                span,
            },
        ],
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::IfPattern(Box::new(TypedIfPattern {
                    target: TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::Local(target_local),
                    },
                    bool_ty: ty,
                    arms: vec![TypedIfPatternArm {
                        pattern: TypedPattern {
                            ty,
                            span,
                            kind: TypedPatternKind::OptionalSome(Box::new(TypedPattern {
                                ty,
                                span,
                                kind: TypedPatternKind::Bind {
                                    local_id: payload_local,
                                    name: "x".to_string(),
                                },
                            })),
                        },
                        body: TypedBody {
                            span,
                            locals: Vec::new(),
                            stmts: Vec::new(),
                            tail: Some(Box::new(TypedExpr {
                                span,
                                ty,
                                kind: TypedExprKind::Local(payload_local),
                            })),
                            ty,
                        },
                        span,
                    }],
                    else_branch: Some(Box::new(int_expr(0))),
                })),
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert!(
        function_body
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::If { .. })),
        "{function_body:#?}"
    );
    assert!(
        function_body.blocks.iter().any(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op,
                    FunctionOp::StoreLocal {
                        local_id,
                        value:
                            FunctionExpr {
                                kind: FunctionExprKind::TaggedUnionPayload { .. },
                                ..
                            },
                        ..
                    } if *local_id == payload_local
                )
            })
        }),
        "{function_body:#?}"
    );
}

#[test]
fn value_if_pattern_caches_target_and_stores_payload_binding() {
    let ty = test_ty();
    let target_local = LocalId(0);
    let payload_local = LocalId(1);
    let span = Span::default();
    let if_expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::IfPattern(Box::new(TypedIfPattern {
            target: TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Local(target_local),
            },
            bool_ty: ty,
            arms: vec![
                TypedIfPatternArm {
                    pattern: TypedPattern {
                        ty,
                        span,
                        kind: TypedPatternKind::OptionalSome(Box::new(TypedPattern {
                            ty,
                            span,
                            kind: TypedPatternKind::Bind {
                                local_id: payload_local,
                                name: "payload".to_string(),
                            },
                        })),
                    },
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: Vec::new(),
                        tail: Some(Box::new(TypedExpr {
                            span,
                            ty,
                            kind: TypedExprKind::Local(payload_local),
                        })),
                        ty,
                    },
                    span,
                },
                TypedIfPatternArm {
                    pattern: TypedPattern {
                        ty,
                        span,
                        kind: TypedPatternKind::OptionalNull,
                    },
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: Vec::new(),
                        tail: Some(Box::new(int_expr(0))),
                        ty,
                    },
                    span,
                },
            ],
            else_branch: None,
        })),
    };
    let body = TypedBody {
        span,
        locals: vec![
            TypedLocal {
                id: target_local,
                name: "value".to_string(),
                kind: TypedLocalKind::Param,
                ty,
                span,
            },
            TypedLocal {
                id: payload_local,
                name: "payload".to_string(),
                kind: TypedLocalKind::Binding,
                ty,
                span,
            },
        ],
        stmts: Vec::new(),
        tail: Some(Box::new(if_expr)),
        ty,
    };

    let function_body = lower_function_body(&body);

    let target_cache_count = function_body
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            matches!(
                op,
                FunctionOp::StoreLocal {
                    value:
                        FunctionExpr {
                            kind: FunctionExprKind::Local(local_id),
                            ..
                        },
                    ..
                } if *local_id == target_local
            )
        })
        .count();
    assert_eq!(target_cache_count, 1, "{function_body:#?}");
    assert!(
        function_body.blocks.iter().any(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op,
                    FunctionOp::StoreLocal {
                        local_id,
                        value:
                            FunctionExpr {
                                kind: FunctionExprKind::TaggedUnionPayload { .. },
                                ..
                            },
                        ..
                    } if *local_id == payload_local
                )
            })
        }),
        "{function_body:#?}"
    );
    assert!(
        function_body
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::If { .. })),
        "{function_body:#?}"
    );
}

#[test]
fn value_if_pattern_trap_else_lowers_as_effect_only() {
    let ty = test_ty();
    let target_local = LocalId(0);
    let payload_local = LocalId(1);
    let span = Span::default();
    let if_expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::IfPattern(Box::new(TypedIfPattern {
            target: TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Local(target_local),
            },
            bool_ty: ty,
            arms: vec![TypedIfPatternArm {
                pattern: TypedPattern {
                    ty,
                    span,
                    kind: TypedPatternKind::OptionalSome(Box::new(TypedPattern {
                        ty,
                        span,
                        kind: TypedPatternKind::Bind {
                            local_id: payload_local,
                            name: "payload".to_string(),
                        },
                    })),
                },
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: Vec::new(),
                    tail: Some(Box::new(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::Local(payload_local),
                    })),
                    ty,
                },
                span,
            }],
            else_branch: Some(Box::new(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Trap,
            })),
        })),
    };
    let body = TypedBody {
        span,
        locals: vec![
            TypedLocal {
                id: target_local,
                name: "value".to_string(),
                kind: TypedLocalKind::Param,
                ty,
                span,
            },
            TypedLocal {
                id: payload_local,
                name: "payload".to_string(),
                kind: TypedLocalKind::Binding,
                ty,
                span,
            },
        ],
        stmts: Vec::new(),
        tail: Some(Box::new(if_expr)),
        ty,
    };

    let function_body = lower_function_body(&body);

    assert!(
        !function_body.blocks.iter().any(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op,
                    FunctionOp::StoreLocal {
                        value: FunctionExpr {
                            kind: FunctionExprKind::Trap,
                            ..
                        },
                        ..
                    }
                )
            })
        }),
        "{function_body:#?}"
    );
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
    validate_function_body(&function_body).expect("trap else should be valid effect IR");
}

#[test]
fn statement_switch_arm_block_exits_arm_scope_to_merge() {
    let body = switch_stmt_body(vec![switch_expr_arm(
        1,
        TypedSwitchArmBody::Block(Box::new(TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Defer(int_expr(1)),
            }],
            tail: None,
            ty: test_ty(),
        })),
    )]);

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Switch { arms, fallback, .. } = &function_body.blocks[0].terminator
    else {
        panic!("expected switch terminator");
    };
    let arm = function_body.block(arms[0].target).expect("arm block");

    assert_eq!(arm.scope, FunctionScopeId(1));
    assert!(matches!(arm.ops[0], FunctionOp::Defer(_)));
    assert_eq!(
        function_body.edge_exited_scopes(arm.id, *fallback),
        Some(vec![FunctionScopeId(1)])
    );
}

#[test]
fn return_from_statement_switch_arm_exits_arm_and_root_scopes() {
    let body = switch_stmt_body(vec![switch_expr_arm(
        1,
        TypedSwitchArmBody::Stmt(Box::new(TypedStmt {
            span: Span::default(),
            kind: TypedStmtKind::Return(Some(int_expr(1))),
        })),
    )]);

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Switch { arms, .. } = &function_body.blocks[0].terminator else {
        panic!("expected switch terminator");
    };

    assert!(matches!(
        function_body
            .block(arms[0].target)
            .expect("arm block")
            .terminator,
        FunctionTerminator::Return { .. }
    ));
    assert_eq!(
        function_body.return_exited_scopes(arms[0].target),
        Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
    );
}

#[test]
fn collects_unique_locals_from_statement_switch_arms() {
    let span = Span::default();
    let ty = test_ty();
    let arm_local = TypedLocal {
        id: LocalId(1),
        name: "arm_local".to_string(),
        kind: TypedLocalKind::Binding,
        ty,
        span,
    };
    let body = switch_stmt_body(vec![switch_expr_arm(
        1,
        TypedSwitchArmBody::Block(Box::new(TypedBody {
            span,
            locals: vec![arm_local],
            stmts: Vec::new(),
            tail: None,
            ty,
        })),
    )]);

    let function_body = lower_function_body(&body);

    assert_eq!(
        function_body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<Vec<_>>(),
        vec![LocalId(1)]
    );
}
