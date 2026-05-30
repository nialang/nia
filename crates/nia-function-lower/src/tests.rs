// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_body_ir::{
    TypedBody, TypedExpr, TypedExprKind, TypedForHeader, TypedForInit, TypedLocal, TypedLocalKind,
    TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_function_ir::*;
use nia_ids::{InternedTyId, LocalId, ModuleId, TyInternerIndex};
use nia_span::Span;

fn only_next_target(function_body: &FunctionBody, block: FunctionBlockId) -> FunctionBlockId {
    let FunctionTerminator::Next { target, .. } = function_body
        .block(block)
        .expect("function block")
        .terminator
    else {
        panic!("expected next terminator");
    };
    target
}

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

#[test]
fn resolves_break_to_loop_exit_branch() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Break,
                    }],
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected entry branch to loop header");
    };
    let FunctionTerminator::Loop {
        body: loop_body,
        break_target,
        ..
    } = function_body.block(target).expect("loop header").terminator
    else {
        panic!("expected loop terminator");
    };
    let loop_body = function_body
        .blocks
        .iter()
        .find(|block| block.id == loop_body)
        .expect("loop body block");

    assert_eq!(loop_body.terminator.successors(), vec![break_target]);
    assert!(matches!(
        loop_body.terminator,
        FunctionTerminator::Branch { .. }
    ));
}

#[test]
fn resolves_continue_to_loop_continue_branch() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Continue,
                    }],
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected entry branch to loop header");
    };
    let FunctionTerminator::Loop {
        body: loop_body,
        continue_target,
        ..
    } = function_body.block(target).expect("loop header").terminator
    else {
        panic!("expected loop terminator");
    };
    let loop_body = function_body
        .blocks
        .iter()
        .find(|block| block.id == loop_body)
        .expect("loop body block");

    assert_eq!(loop_body.terminator.successors(), vec![continue_target]);
    assert!(matches!(
        loop_body.terminator,
        FunctionTerminator::Branch { .. }
    ));
}

#[test]
fn lowers_c_style_for_init_step_and_edges() {
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
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::CStyle {
                    init: Some(Box::new(TypedForInit::Expr(expr.clone()))),
                    cond: Some(Box::new(expr.clone())),
                    step: Some(Box::new(expr)),
                },
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert!(matches!(
        function_body.blocks[0].ops[0],
        FunctionOp::Expr(_)
    ));
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Next { .. }
    ));
    let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
    let loop_target = only_next_target(&function_body, loop_target);
    let loop_block = function_body.block(loop_target).expect("loop header");
    let FunctionTerminator::Loop {
        body,
        continue_target,
        break_target,
        ..
    } = loop_block.terminator
    else {
        panic!("expected loop terminator");
    };
    assert_eq!(loop_block.terminator.successors(), vec![body, break_target]);
    let continue_block = function_body
        .blocks
        .iter()
        .find(|block| block.id == continue_target)
        .expect("continue block");
    assert!(matches!(continue_block.ops[0], FunctionOp::Expr(_)));
    let step_branch = only_next_target(&function_body, continue_block.id);
    assert_eq!(
        function_body
            .block(step_branch)
            .expect("step branch block")
            .terminator
            .successors(),
        vec![loop_block.id]
    );
}

#[test]
fn loop_body_gets_child_scope_with_parent_loop_edges() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);
    let root_scope = FunctionScopeId(0);
    let loop_scope = FunctionScopeId(1);
    let loop_target = only_next_target(&function_body, function_body.blocks[0].id);
    let FunctionTerminator::Loop {
        body,
        continue_target,
        break_target,
        ..
    } = function_body
        .block(loop_target)
        .expect("loop header")
        .terminator
    else {
        panic!("expected loop terminator");
    };
    let body_block = function_body
        .blocks
        .iter()
        .find(|block| block.id == body)
        .expect("loop body block");
    let continue_block = function_body
        .blocks
        .iter()
        .find(|block| block.id == continue_target)
        .expect("continue block");
    let break_block = function_body
        .blocks
        .iter()
        .find(|block| block.id == break_target)
        .expect("break block");

    assert_eq!(function_body.scopes[0].parent, None);
    assert_eq!(function_body.scopes[1].parent, Some(root_scope));
    assert_eq!(function_body.blocks[0].scope, root_scope);
    assert_eq!(
        function_body.block(loop_target).expect("loop header").scope,
        root_scope
    );
    assert_eq!(body_block.scope, loop_scope);
    assert_eq!(continue_block.scope, root_scope);
    assert_eq!(break_block.scope, root_scope);
}

#[test]
fn preserves_unique_locals_from_flattened_loop_bodies() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let outer_local = TypedLocal {
        id: LocalId(0),
        name: "outer".to_string(),
        kind: TypedLocalKind::Binding,
        ty,
        span,
    };
    let inner_local = TypedLocal {
        id: LocalId(1),
        name: "inner".to_string(),
        kind: TypedLocalKind::Binding,
        ty,
        span,
    };
    let body = TypedBody {
        span,
        locals: vec![outer_local, inner_local.clone()],
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: vec![inner_local],
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert_eq!(
        function_body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<Vec<_>>(),
        vec![LocalId(0), LocalId(1)]
    );
}

#[test]
fn nested_loops_resolve_break_and_continue_to_nearest_loop() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let inner_continue_loop = TypedStmt {
        span,
        kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
            header: TypedForHeader::Infinite,
            body: TypedBody {
                span,
                locals: Vec::new(),
                stmts: vec![TypedStmt {
                    span,
                    kind: TypedStmtKind::Continue,
                }],
                tail: None,
                ty,
            },
        })),
    };
    let inner_break_loop = TypedStmt {
        span,
        kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
            header: TypedForHeader::Infinite,
            body: TypedBody {
                span,
                locals: Vec::new(),
                stmts: vec![TypedStmt {
                    span,
                    kind: TypedStmtKind::Break,
                }],
                tail: None,
                ty,
            },
        })),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![inner_continue_loop, inner_break_loop],
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);
    let outer_loop = only_next_target(&function_body, function_body.blocks[0].id);
    let FunctionTerminator::Loop {
        body: outer_body, ..
    } = function_body
        .block(outer_loop)
        .expect("outer loop")
        .terminator
    else {
        panic!("expected outer loop");
    };
    let outer_body = function_body
        .blocks
        .iter()
        .find(|block| block.id == outer_body)
        .expect("outer body block");
    let first_inner_loop = only_next_target(&function_body, outer_body.id);
    let FunctionTerminator::Loop {
        body: inner_body,
        continue_target: inner_continue,
        break_target: first_inner_break,
        ..
    } = function_body
        .block(first_inner_loop)
        .expect("first inner loop")
        .terminator
    else {
        panic!("expected first inner loop");
    };
    let inner_body = function_body
        .blocks
        .iter()
        .find(|block| block.id == inner_body)
        .expect("first inner body block");

    assert_eq!(inner_body.terminator.successors(), vec![inner_continue]);

    let second_inner_loop = only_next_target(&function_body, first_inner_break);
    let second_inner_loop = function_body
        .block(second_inner_loop)
        .expect("second inner loop");
    let FunctionTerminator::Loop {
        body: inner_body,
        break_target: inner_break,
        ..
    } = second_inner_loop.terminator
    else {
        panic!("expected second inner loop");
    };
    let inner_body = function_body
        .blocks
        .iter()
        .find(|block| block.id == inner_body)
        .expect("second inner body block");

    assert_eq!(inner_body.terminator.successors(), vec![inner_break]);
}

#[test]
fn nested_loop_scopes_preserve_parent_chain() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                            header: TypedForHeader::Infinite,
                            body: TypedBody {
                                span,
                                locals: Vec::new(),
                                stmts: Vec::new(),
                                tail: None,
                                ty,
                            },
                        })),
                    }],
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert_eq!(
        function_body
            .scopes
            .iter()
            .map(|scope| (scope.id, scope.parent))
            .collect::<Vec<_>>(),
        vec![
            (FunctionScopeId(0), None),
            (FunctionScopeId(1), Some(FunctionScopeId(0))),
            (FunctionScopeId(2), Some(FunctionScopeId(1))),
        ]
    );
}

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

    let function_body = lower_function_body(&body);

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
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                header: TypedForHeader::Infinite,
                body: TypedBody {
                    span,
                    locals: Vec::new(),
                    stmts: vec![TypedStmt {
                        span,
                        kind: TypedStmtKind::Break,
                    }],
                    tail: None,
                    ty,
                },
            })),
        }],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);
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

fn manual_function_body_for_scope_edges() -> FunctionBody {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    FunctionBody {
        span,
        locals: Vec::new(),
        scopes: vec![
            FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            },
            FunctionScope {
                id: FunctionScopeId(1),
                parent: Some(FunctionScopeId(0)),
                span,
            },
            FunctionScope {
                id: FunctionScopeId(2),
                parent: Some(FunctionScopeId(0)),
                span,
            },
        ],
        blocks: vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(1),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(2),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail { value: None, span },
            },
        ],
        entry: FunctionBlockId(0),
        ty,
    }
}

#[test]
fn lowers_statement_block_expression_into_child_scope() {
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

    let function_body = lower_function_body(&body);

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

    let function_body = lower_function_body(&body);

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
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let inner_local = TypedLocal {
        id: LocalId(1),
        name: "inner".to_string(),
        kind: TypedLocalKind::Binding,
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

#[test]
fn collects_unique_locals_from_statement_if_arms() {
    let span = Span::default();
    let ty = test_ty();
    let then_local = TypedLocal {
        id: LocalId(1),
        name: "then_local".to_string(),
        kind: TypedLocalKind::Binding,
        ty,
        span,
    };
    let else_local = TypedLocal {
        id: LocalId(2),
        name: "else_local".to_string(),
        kind: TypedLocalKind::Binding,
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

    let function_body = lower_function_body(&body);

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

    let function_body = lower_function_body(&body);
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

    let function_body = lower_function_body(&body);
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

    let function_body = lower_function_body(&body);
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

    let function_body = lower_function_body(&body);
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

fn test_ty() -> InternedTyId {
    InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0))
}

fn int_expr(value: i32) -> TypedExpr {
    TypedExpr {
        span: Span::default(),
        ty: test_ty(),
        kind: TypedExprKind::Integer(value.to_string()),
    }
}

fn bool_expr(value: bool) -> TypedExpr {
    TypedExpr {
        span: Span::default(),
        ty: test_ty(),
        kind: TypedExprKind::Bool(value),
    }
}

fn empty_body(ty: InternedTyId) -> TypedBody {
    TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: None,
        ty,
    }
}

fn switch_stmt_body(arms: Vec<nia_body_ir::TypedSwitchArm>) -> TypedBody {
    let span = Span::default();
    let ty = test_ty();
    TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Switch(Box::new(TypedSwitch {
                    target: int_expr(1),
                    arms,
                })),
            }),
        }],
        tail: None,
        ty,
    }
}

fn switch_expr_arm(value: i32, body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
    nia_body_ir::TypedSwitchArm {
        pattern: TypedSwitchPattern::Expr(int_expr(value)),
        body,
        span: Span::default(),
    }
}

fn switch_default_arm(body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
    nia_body_ir::TypedSwitchArm {
        pattern: TypedSwitchPattern::Default,
        body,
        span: Span::default(),
    }
}
