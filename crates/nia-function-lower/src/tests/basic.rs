// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn lowers_body_to_entry_block_with_tail() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: "x".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        })),
        ty,
    };

    let function_body = lower_function_body(&body);

    assert_eq!(function_body.entry, FunctionBlockId(0));
    assert_eq!(function_body.blocks.len(), 1);
    assert!(function_body.blocks[0].ops.is_empty());
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Tail { value: Some(_), .. }
    ));
}

#[test]
fn non_terminal_ops_branch_to_tail_block() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
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
            kind: TypedStmtKind::Expr(expr.clone()),
        }],
        tail: Some(Box::new(expr)),
        ty,
    };

    let function_body = lower_function_body(&body);

    assert_eq!(function_body.blocks.len(), 2);
    assert_eq!(function_body.blocks[0].ops.len(), 1);
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Next {
            target: FunctionBlockId(1),
            ..
        }
    ));
    assert_eq!(
        function_body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(1)]
    );
    assert!(matches!(
        function_body.blocks[1].terminator,
        FunctionTerminator::Tail { value: Some(_), .. }
    ));
}

#[test]
fn lowers_address_of_places_to_function_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: "x".to_string(),
            kind: TypedLocalKind::Binding,
            ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                expr: Box::new(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Local(LocalId(0)),
                }),
            },
        })),
        ty,
    };

    let function_body = lower_function_body(&body);

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &function_body.blocks[0].terminator
    else {
        panic!("expected address-of tail value");
    };
    let FunctionExprKind::AddrOf(place) = &value.kind else {
        panic!("expected address-of place");
    };
    assert!(matches!(place.base, FunctionPlaceBase::Local(LocalId(0))));
}

#[test]
fn return_terminates_block_before_later_statements() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::Integer("1".to_string()),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![
            TypedStmt {
                span,
                kind: TypedStmtKind::Return(Some(expr.clone())),
            },
            TypedStmt {
                span,
                kind: TypedStmtKind::Expr(expr),
            },
        ],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert_eq!(function_body.blocks.len(), 1);
    assert!(function_body.blocks[0].ops.is_empty());
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Return { value: Some(_), .. }
    ));
}
