// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn lowers_statement_block_expression_into_child_scope() {
    let span = Span::default();
    let ty = test_ty();
    let expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::Integer("1".to_string()),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Block(TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Defer(expr.clone()),
                    }],
                    tail: Some(Box::new(expr)),
                    ty,
                }),
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(function_body.scopes[1].parent, Some(FunctionScopeId(0)));
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Next {
            target: FunctionBlockId(1),
            ..
        }
    ));
    assert_eq!(function_body.blocks[1].scope, FunctionScopeId(1));
    assert!(matches!(
        function_body.blocks[1].ops[0],
        FunctionOp::Defer(_)
    ));
    assert!(matches!(
        function_body.blocks[1].ops[1],
        FunctionOp::Expr(_)
    ));
    assert_eq!(
        function_body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
        Some(vec![FunctionScopeId(1)])
    );
    assert!(!function_body.blocks[0].ops.iter().any(|op| matches!(
        op,
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Local(_),
            ..
        })
    )));
}

#[test]
fn return_from_statement_block_exits_block_and_root_scopes() {
    let span = Span::default();
    let ty = test_ty();
    let expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::Integer("1".to_string()),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Block(TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Return(Some(expr)),
                    }],
                    tail: None,
                    ty,
                }),
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert!(matches!(
        function_body.blocks[1].terminator,
        FunctionTerminator::Return { .. }
    ));
    assert_eq!(
        function_body.return_exited_scopes(FunctionBlockId(1)),
        Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
    );
}

#[test]
fn collects_unique_locals_from_statement_block_expressions() {
    let span = Span::default();
    let ty = test_ty();
    let inner_local = TypedLocal {
        id: LocalId(1),
        name: local_name("inner"),
        kind: TypedLocalKind::MutableBinding,
        ty,
        span,
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Block(TypedBody {
                    span,
                    locals: vec![inner_local],
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                }),
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(
        function_body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<Vec<_>>(),
        vec![LocalId(1)]
    );
}

#[test]
fn collects_locals_from_deferred_block_expressions() {
    let span = Span::default();
    let ty = test_ty();
    let deferred_local = TypedLocal {
        id: LocalId(3),
        name: local_name("deferred_local"),
        kind: TypedLocalKind::MutableBinding,
        ty,
        span,
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Defer(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Block(TypedBody {
                    span,
                    locals: vec![deferred_local],
                    stmts: Vec::new(),
                    tail: Some(Box::new(int_expr(1))),
                    ty,
                }),
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert!(function_body.locals.iter().any(|local| {
        local.id == LocalId(3)
            && local.name == local_name("deferred_local")
            && local.kind == FunctionLocalKind::MutableBinding
    }));
    validate_function_body(&function_body).expect("valid defer block local table");
}

#[test]
fn collects_unique_locals_from_statement_if_arms() {
    let span = Span::default();
    let ty = test_ty();
    let then_local = TypedLocal {
        id: LocalId(1),
        name: local_name("then_local"),
        kind: TypedLocalKind::MutableBinding,
        ty,
        span,
    };
    let else_local = TypedLocal {
        id: LocalId(2),
        name: local_name("else_local"),
        kind: TypedLocalKind::MutableBinding,
        ty,
        span,
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::If {
                    cond: Box::new(bool_expr(true)),
                    then_branch: TypedBody {
                        span,
                        locals: vec![then_local],
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    },
                    else_branch: Some(Box::new(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::Block(TypedBody {
                            span,
                            locals: vec![else_local],
                            stmts: Vec::new(),
                            tail: None,
                            ty,
                        }),
                    })),
                },
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(
        function_body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<Vec<_>>(),
        vec![LocalId(1), LocalId(2)]
    );
}

#[test]
fn lowers_statement_if_into_if_terminator_and_child_scope() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::If {
                    cond: Box::new(bool_expr(true)),
                    then_branch: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Defer(int_expr(1)),
                        }],
                        tail: None,
                        ty,
                    },
                    else_branch: None,
                },
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");
    let FunctionTerminator::If {
        then_target,
        else_target,
        ..
    } = function_body.blocks[0].terminator
    else {
        panic!("expected if terminator");
    };

    assert_eq!(
        function_body.blocks[0].terminator.successors(),
        vec![then_target, else_target]
    );
    assert_eq!(then_target, FunctionBlockId(1));
    assert_eq!(else_target, FunctionBlockId(2));
    assert_eq!(
        function_body
            .scope(function_body.block(then_target).expect("then block").scope)
            .unwrap()
            .parent,
        Some(FunctionScopeId(0))
    );
    assert!(matches!(
        function_body.block(then_target).expect("then block").ops[0],
        FunctionOp::Defer(_)
    ));
}

#[test]
fn statement_if_without_else_uses_merge_as_false_edge() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![
            TypedStmt {
                span,
                kind: TypedStmtKind::Expr(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::If {
                        cond: Box::new(bool_expr(true)),
                        then_branch: empty_body(ty),
                        else_branch: None,
                    },
                }),
            },
            TypedStmt {
                span,
                kind: TypedStmtKind::Expr(int_expr(1)),
            },
        ],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");
    let FunctionTerminator::If { else_target, .. } = function_body.blocks[0].terminator else {
        panic!("expected if terminator");
    };
    let merge = function_body.block(else_target).expect("merge block");

    assert_eq!(merge.scope, FunctionScopeId(0));
    assert!(matches!(merge.ops[0], FunctionOp::Expr(_)));
}

#[test]
fn statement_if_with_else_block_exits_else_scope_to_merge() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::If {
                    cond: Box::new(bool_expr(true)),
                    then_branch: empty_body(ty),
                    else_branch: Some(Box::new(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::Block(TypedBody {
                            span,
                            locals: Vec::new(),
                            stmts: vec![TypedStmt {
                                span,
                                kind: TypedStmtKind::Defer(int_expr(2)),
                            }],
                            tail: None,
                            ty,
                        }),
                    })),
                },
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");
    let FunctionTerminator::If { else_target, .. } = function_body.blocks[0].terminator else {
        panic!("expected if terminator");
    };
    let else_entry = function_body.block(else_target).expect("else entry block");
    let FunctionTerminator::Next {
        target: else_body, ..
    } = else_entry.terminator
    else {
        panic!("expected else block jump");
    };
    let else_body = function_body.block(else_body).expect("else body block");
    let merge = function_body
        .blocks
        .iter()
        .find(|block| block.scope == FunctionScopeId(0) && block.id.0 > else_body.id.0)
        .expect("merge block");

    assert_eq!(else_body.scope, FunctionScopeId(2));
    assert_eq!(
        function_body.edge_exited_scopes(else_body.id, merge.id),
        Some(vec![FunctionScopeId(2)])
    );
}

#[test]
fn return_from_statement_if_arm_exits_arm_and_root_scopes() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::If {
                    cond: Box::new(bool_expr(true)),
                    then_branch: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Return(Some(int_expr(1))),
                        }],
                        tail: None,
                        ty,
                    },
                    else_branch: None,
                },
            }),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");
    let FunctionTerminator::If { then_target, .. } = function_body.blocks[0].terminator else {
        panic!("expected if terminator");
    };

    assert!(matches!(
        function_body
            .block(then_target)
            .expect("then block")
            .terminator,
        FunctionTerminator::Return { .. }
    ));
    assert_eq!(
        function_body.return_exited_scopes(then_target),
        Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
    );
}
