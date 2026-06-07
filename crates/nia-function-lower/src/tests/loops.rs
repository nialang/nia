// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn resolves_break_to_loop_exit_branch() {
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
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Continue,
            }],
            tail: None,
            ty,
        })],
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
fn lowers_for_in_iterator_next_payload_and_edges() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![for_iterator_stmt(
            LocalId(0),
            TypedBody {
                span,
                locals: Vec::new(),
                stmts: Vec::new(),
                tail: None,
                ty,
            },
        )],
        tail: None,
        ty,
    };

    let function_body = lower_function_body(&body);

    assert!(matches!(
        function_body.blocks[0].ops[0],
        FunctionOp::Binding(_)
    ));
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Next { .. }
    ));
    let next_block = function_body
        .blocks
        .iter()
        .find(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op,
                    FunctionOp::Binding(binding) if binding.name == "__for_next"
                )
            })
        })
        .expect("iterator next block");
    assert!(
        function_body
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .any(|op| {
                matches!(
                    op,
                    FunctionOp::Binding(binding) if binding.name == "__for_iter" && !binding.is_let
                )
            })
    );
    assert!(next_block.ops.iter().any(|op| {
        matches!(
            op,
            FunctionOp::Binding(binding)
                if binding.name == "__for_next"
                    && matches!(
                        binding.value.as_ref().map(|value| &value.kind),
                        Some(FunctionExprKind::Call {
                            callee: FunctionCallee::BuiltinPlaceMethod {
                                trait_id: nia_ids::BuiltinTrait::Iterator,
                                method: nia_ids::BuiltinTraitMethod::IteratorNext,
                                ..
                            },
                            ..
                        })
                    )
        )
    }));
    let loop_block = function_body
        .block(only_next_target(&function_body, next_block.id))
        .expect("loop header");
    let FunctionTerminator::Loop {
        body,
        continue_target,
        break_target,
        header,
        ..
    } = &loop_block.terminator
    else {
        panic!("expected loop terminator");
    };
    assert!(matches!(header, FunctionForHeader::Condition(_)));
    assert_eq!(
        loop_block.terminator.successors(),
        vec![*body, *break_target]
    );
    let body_block = function_body.block(*body).expect("loop body block");
    assert!(body_block.ops.iter().any(|op| {
        matches!(
            op,
            FunctionOp::Binding(binding)
                if matches!(
                    binding.value.as_ref().map(|value| &value.kind),
                    Some(FunctionExprKind::TaggedUnionPayload { .. })
                )
        )
    }));
    let continue_block = function_body
        .blocks
        .iter()
        .find(|block| block.id == *continue_target)
        .expect("continue block");
    assert!(
        continue_block
            .terminator
            .successors()
            .contains(&next_block.id)
    );
}

#[test]
fn loop_body_gets_child_scope_with_parent_loop_edges() {
    let span = Span::default();
    let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: Vec::new(),
            stmts: Vec::new(),
            tail: None,
            ty,
        })],
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
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: vec![inner_local],
            stmts: Vec::new(),
            tail: None,
            ty,
        })],
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
    let inner_continue_loop = loop_stmt(TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Continue,
        }],
        tail: None,
        ty,
    });
    let inner_break_loop = loop_stmt(TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Break,
        }],
        tail: None,
        ty,
    });
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![inner_continue_loop, inner_break_loop],
            tail: None,
            ty,
        })],
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
        stmts: vec![loop_stmt(TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![loop_stmt(TypedBody {
                span,
                locals: Vec::new(),
                stmts: Vec::new(),
                tail: None,
                ty,
            })],
            tail: None,
            ty,
        })],
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
