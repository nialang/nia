// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_body_ir::{
    TypedBody, TypedExpr, TypedExprKind, TypedForBinding, TypedForIn, TypedLocal, TypedLocalKind,
    TypedLoop, TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
};
pub(super) use nia_function_ir::*;
pub(super) use nia_ids::{InternedTyId, LocalId, ModuleId, TyInternerIndex};
pub(super) use nia_span::Span;
pub(super) use nia_ty::{PrimitiveTy, TyInterner, TyKind};

pub(super) fn only_next_target(
    function_body: &FunctionBody,
    block: FunctionBlockId,
) -> FunctionBlockId {
    let FunctionTerminator::Next { target, .. } = function_body
        .block(block)
        .expect("function block")
        .terminator
    else {
        panic!("expected next terminator");
    };
    target
}

pub(super) fn test_ty() -> InternedTyId {
    InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0))
}

pub(super) fn int_expr(value: i32) -> TypedExpr {
    TypedExpr {
        span: Span::default(),
        ty: test_ty(),
        kind: TypedExprKind::Integer(value.to_string()),
    }
}

pub(super) fn bool_expr(value: bool) -> TypedExpr {
    TypedExpr {
        span: Span::default(),
        ty: test_ty(),
        kind: TypedExprKind::Bool(value),
    }
}

pub(super) fn empty_body(ty: InternedTyId) -> TypedBody {
    TypedBody {
        span: Span::default(),
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: None,
        ty,
    }
}

pub(super) fn loop_stmt(body: TypedBody) -> TypedStmt {
    TypedStmt {
        span: Span::default(),
        kind: TypedStmtKind::Loop(Box::new(TypedLoop { body })),
    }
}

pub(super) fn for_iterator_stmt(local_id: LocalId, body: TypedBody) -> TypedStmt {
    let span = Span::default();
    let ty = test_ty();
    TypedStmt {
        span,
        kind: TypedStmtKind::ForIn(Box::new(TypedForIn {
            binding: Some(TypedForBinding {
                local_id,
                name: "i".to_string(),
            }),
            pattern_kind: nia_ast::ForPatternKind::Value,
            item_ty: ty,
            binding_ty: ty,
            iter: TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Local(LocalId(10)),
            },
            body,
        })),
    }
}

pub(super) fn switch_stmt_body(arms: Vec<nia_body_ir::TypedSwitchArm>) -> TypedBody {
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
                    bool_ty: ty,
                    arms,
                })),
            }),
        }],
        tail: None,
        ty,
    }
}

pub(super) fn switch_expr_arm(value: i32, body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
    nia_body_ir::TypedSwitchArm {
        patterns: vec![TypedSwitchPattern::CheckedInt {
            value: value.into(),
            ty: test_ty(),
            span: Span::default(),
        }],
        body,
        span: Span::default(),
    }
}

pub(super) fn switch_range_arm(
    start: i32,
    end: i32,
    inclusive: bool,
    body: TypedSwitchArmBody,
) -> nia_body_ir::TypedSwitchArm {
    nia_body_ir::TypedSwitchArm {
        patterns: vec![TypedSwitchPattern::CheckedIntRange {
            start: start.into(),
            end: end.into(),
            inclusive,
            ty: test_ty(),
            span: Span::default(),
        }],
        body,
        span: Span::default(),
    }
}

pub(super) fn switch_default_arm(body: TypedSwitchArmBody) -> nia_body_ir::TypedSwitchArm {
    nia_body_ir::TypedSwitchArm {
        patterns: vec![TypedSwitchPattern::Default],
        body,
        span: Span::default(),
    }
}

pub(super) fn manual_function_body_for_scope_edges() -> FunctionBody {
    let span = Span::default();
    let ty = test_ty();
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
