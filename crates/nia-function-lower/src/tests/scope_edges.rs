// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn same_scope_edges_exit_no_scopes() {
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
            kind: TypedStmtKind::Expr(expr),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body).expect("valid typed body");

    assert_eq!(
        function_body.edge_exited_scopes(FunctionBlockId(0), FunctionBlockId(1)),
        Some(Vec::new())
    );
}

#[test]
fn loop_body_break_edge_exits_loop_scope() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Break,
            }],
            tail: None,
            ty,
        })],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body).expect("valid typed body");
    let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
    let FunctionTerminator::Loop {
        body, break_target, ..
    } = function_body
        .block(loop_target)
        .expect("loop header")
        .terminator
    else {
        panic!("expected loop terminator");
    };

    assert_eq!(
        function_body.edge_exited_scopes(body, break_target),
        Some(vec![FunctionScopeId(1)])
    );
}

#[test]
fn sibling_scope_edge_exits_only_source_scope() {
    let body = manual_function_body_for_scope_edges();

    assert_eq!(
        body.exited_scopes_between(FunctionScopeId(1), Some(FunctionScopeId(2))),
        Some(vec![FunctionScopeId(1)])
    );
}

#[test]
fn return_edge_exits_scope_chain_to_function_boundary() {
    let body = manual_function_body_for_scope_edges();

    assert_eq!(
        body.return_exited_scopes(FunctionBlockId(1)),
        Some(vec![FunctionScopeId(1), FunctionScopeId(0)])
    );
}
