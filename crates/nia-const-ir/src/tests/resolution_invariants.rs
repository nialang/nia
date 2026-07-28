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
