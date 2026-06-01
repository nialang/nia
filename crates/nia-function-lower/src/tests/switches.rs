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
