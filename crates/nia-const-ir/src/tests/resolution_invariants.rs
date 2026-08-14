use super::*;

#[test]
fn resolved_expr_rejects_unresolved_names() {
    let expr = EarlyConstExpr {
        span: span(),
        kind: EarlyConstExprKind::Ident(EarlyConstName::unresolved(sym("x"))),
    };

    let err = ResolvedConstExpr::new(expr).expect_err("unresolved name must be rejected");
    assert_eq!(err.message, "failed to resolve const name");
}

#[test]
fn resolved_expr_rejects_unresolved_assignment_targets() {
    let expr = EarlyConstExpr {
        span: span(),
        kind: EarlyConstExprKind::Assign(Box::new(EarlyConstAssign {
            lhs: EarlyConstAssignTarget::Local {
                span: span(),
                name: sym("x"),
                local_id: None,
                path: Vec::new(),
            },
            op: ConstAssignOp::Assign,
            rhs: int_expr("1"),
        })),
    };

    let err =
        ResolvedConstExpr::new(expr).expect_err("unresolved assignment target must be rejected");
    assert_eq!(err.message, "failed to resolve const assignment target");
}

#[test]
fn resolved_function_rejects_unresolved_locals() {
    let function = EarlyConstFunction {
        span: span(),
        params: vec![EarlyConstParam {
            span: span(),
            name: sym("x"),
            local_id: None,
            ty: None,
            receiver: None,
        }],
        body: EarlyConstBlock {
            span: span(),
            stmts: Vec::new(),
            tail: None,
        },
    };

    let err = ResolvedConstFunction::new(function)
        .expect_err("unresolved function parameter must be rejected");
    assert_eq!(
        err.message,
        "failed to resolve const function parameter local"
    );
}

#[test]
fn resolved_function_preserves_mutable_receiver_kind() {
    let function = EarlyConstFunction {
        span: span(),
        params: vec![EarlyConstParam {
            span: span(),
            name: sym("self"),
            local_id: Some(LocalId(0)),
            ty: None,
            receiver: Some(nia_ids::ReceiverKind::Ref),
        }],
        body: EarlyConstBlock {
            span: span(),
            stmts: Vec::new(),
            tail: Some(Box::new(int_expr("1"))),
        },
    };

    let function = ResolvedConstFunction::new(function).expect("resolved const function");
    assert_eq!(
        function.params()[0].receiver(),
        Some(nia_ids::ReceiverKind::Ref)
    );
}

#[test]
fn resolved_expr_rejects_unresolved_type_args() {
    let expr = EarlyConstExpr {
        span: span(),
        kind: EarlyConstExprKind::LayoutBuiltin {
            builtin: LayoutBuiltin::Size,
            type_arg: EarlyConstTypeArg {
                span: span(),
                ty_span: span(),
                ty: None,
            },
        },
    };

    let err = ResolvedConstExpr::new(expr).expect_err("unresolved type arg must be rejected");
    assert_eq!(err.message, "failed to resolve const type argument");
}

#[test]
fn resolved_expr_preserves_context_inferred_array_literal() {
    let expr = EarlyConstExpr {
        span: span(),
        kind: EarlyConstExprKind::ArrayLiteral {
            elems: crate::EarlyConstArrayElements::List(Vec::new()),
        },
    };

    let resolved = resolve_expr(expr).expect("untyped array literal should remain inferable");
    assert!(matches!(
        resolved.kind(),
        ResolvedConstExprKind::ArrayLiteral { .. }
    ));
}

#[test]
fn resolved_function_rejects_unresolved_explicit_parameter_type() {
    let function = EarlyConstFunction {
        span: span(),
        params: vec![EarlyConstParam {
            span: span(),
            name: sym("value"),
            local_id: Some(LocalId(0)),
            ty: Some(EarlyConstTypeArg {
                span: span(),
                ty_span: other_span(),
                ty: None,
            }),
            receiver: None,
        }],
        body: EarlyConstBlock {
            span: span(),
            stmts: Vec::new(),
            tail: None,
        },
    };

    let err =
        ResolvedConstFunction::new(function).expect_err("explicit parameter type must resolve");
    assert_eq!(err.span, other_span());
    assert_eq!(
        err.message,
        "failed to resolve const function parameter type"
    );
}

#[test]
fn resolved_function_rejects_unresolved_explicit_binding_type() {
    let function = EarlyConstFunction {
        span: span(),
        params: Vec::new(),
        body: EarlyConstBlock {
            span: span(),
            stmts: vec![crate::EarlyConstStmt {
                span: span(),
                kind: crate::EarlyConstStmtKind::Binding(crate::EarlyConstBinding {
                    span: span(),
                    name: sym("value"),
                    local_id: Some(LocalId(0)),
                    explicit_type: Some(EarlyConstTypeArg {
                        span: span(),
                        ty_span: other_span(),
                        ty: None,
                    }),
                    is_mutable: false,
                    value: int_expr("1"),
                }),
            }],
            tail: None,
        },
    };

    let err = ResolvedConstFunction::new(function).expect_err("explicit binding type must resolve");
    assert_eq!(err.span, other_span());
    assert_eq!(err.message, "failed to resolve const local binding type");
}
