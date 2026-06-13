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

    let function_body = lower_function_body(&body);

    validate_function_body(&function_body).expect("valid function body");
}

#[test]
fn validates_defer_body_references_to_enclosing_locals() {
    let span = Span::default();
    let ty = test_ty();
    let local = TypedLocal {
        id: LocalId(1),
        name: "value".to_string(),
        kind: TypedLocalKind::Binding,
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

    let function_body = lower_function_body(&body);

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

    let function_body = lower_function_body(&body);

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
