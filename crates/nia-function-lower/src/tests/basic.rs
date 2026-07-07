// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn lowers_body_to_entry_block_with_tail() {
    let span = Span::default();
    let ty = InternedTyId::new(
        nia_ids::TyInternerId::for_module(ModuleId(0)),
        TyInternerIndex::from_interner_index(0),
    );
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("x"),
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

    let function_body = lower_function_body(&body).expect("valid typed body");

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
    let ty = InternedTyId::new(
        nia_ids::TyInternerId::for_module(ModuleId(0)),
        TyInternerIndex::from_interner_index(0),
    );
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

    let function_body = lower_function_body(&body).expect("valid typed body");

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
fn lowers_try_expression_to_try_terminator_and_success_local() {
    let span = Span::default();
    let mut interner = TyInterner::new(ModuleId(0));
    let i32_ty = interner.primitive(PrimitiveTy::I32);
    let optional_i32 = interner.intern(TyKind::Optional { elem: i32_ty });
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("value"),
            kind: TypedLocalKind::Binding,
            ty: optional_i32,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty: i32_ty,
            kind: TypedExprKind::Try {
                expr: Box::new(TypedExpr {
                    span,
                    ty: optional_i32,
                    kind: TypedExprKind::Local(LocalId(0)),
                }),
            },
        })),
        ty: i32_ty,
    };

    let function_body = lower_function_body_with_interner(ModuleId(0), &body, &interner)
        .expect("valid typed body")
        .body;

    assert!(function_body.blocks.iter().any(|block| {
        matches!(
            block.terminator,
            FunctionTerminator::Try {
                kind: FunctionTryKind::Optional,
                ..
            }
        )
    }));
    assert!(
        !function_body.blocks.iter().any(|block| matches!(
            &block.terminator,
            FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    kind: FunctionExprKind::Try { .. },
                    ..
                }),
                ..
            }
        )),
        "{function_body:?}"
    );
}

#[test]
fn lowers_address_of_places_to_function_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("x"),
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

    let function_body = lower_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected address-of tail value");
    };
    let FunctionExprKind::AddrOf(place) = &value.kind else {
        panic!("expected address-of place");
    };
    assert!(matches!(place.base, FunctionPlaceBase::Local(LocalId(0))));
}

#[test]
fn address_of_rvalue_materializes_temp_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                expr: Box::new(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Integer("1".to_string()),
                }),
            },
        })),
        ty,
    };

    let function_body = lower_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected address-of tail value");
    };
    let FunctionExprKind::AddrOf(place) = &value.kind else {
        panic!("expected address-of place");
    };
    let FunctionPlaceBase::Local(temp) = place.base else {
        panic!("expected materialized temp local");
    };
    assert_eq!(function_body.locals.len(), 1);
    assert_eq!(function_body.locals[0].id, temp);
    let materialize_block = function_body
        .blocks
        .iter()
        .find(|block| !block.ops.is_empty())
        .expect("expected materialization block");
    assert_eq!(materialize_block.ops.len(), 1);
    let FunctionOp::Binding(binding) = &materialize_block.ops[0] else {
        panic!("expected temp binding");
    };
    assert_eq!(binding.local_id, temp);
    assert!(matches!(
        binding.value.as_ref().map(|value| &value.kind),
        Some(FunctionExprKind::Integer(_))
    ));
}

#[test]
fn address_of_slice_lowers_to_slice_value_not_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("ptr"),
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
                    kind: TypedExprKind::Slice {
                        lhs: Box::new(TypedExpr {
                            span,
                            ty,
                            kind: TypedExprKind::Local(LocalId(0)),
                        }),
                        range: nia_body_ir::TypedSliceRange {
                            start: Some(Box::new(int_expr(0))),
                            end: None,
                            inclusive: false,
                        },
                        is_readonly: false,
                    },
                }),
            },
        })),
        ty,
    };

    let function_body = lower_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected slice tail value");
    };
    assert!(
        matches!(value.kind, FunctionExprKind::Slice { .. }),
        "{value:?}"
    );
}

#[test]
fn return_terminates_block_before_later_statements() {
    let span = Span::default();
    let ty = InternedTyId::new(
        nia_ids::TyInternerId::for_module(ModuleId(0)),
        TyInternerIndex::from_interner_index(0),
    );
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

    let function_body = lower_function_body(&body).expect("valid typed body");

    assert_eq!(function_body.blocks.len(), 1);
    assert!(function_body.blocks[0].ops.is_empty());
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Return { value: Some(_), .. }
    ));
}
