// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_body_ir::{
    TypedBody, TypedExpr, TypedExprKind, TypedForIn, TypedForIterator, TypedLocal, TypedLocalKind,
    TypedLoop, TypedRangeIterator, TypedRangeIteratorKind, TypedStmt, TypedStmtKind, TypedSwitch,
    TypedSwitchArmBody, TypedSwitchPattern,
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

pub(super) fn for_range_stmt(local_id: LocalId, body: TypedBody) -> TypedStmt {
    let span = Span::default();
    let ty = test_ty();
    TypedStmt {
        span,
        kind: TypedStmtKind::ForIn(Box::new(TypedForIn {
            local_id,
            name: "i".to_string(),
            ty,
            iter: TypedForIterator::Range(TypedRangeIterator {
                span,
                ty,
                expr: TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Range(TypedRange {
                        start: Some(Box::new(int_expr(0))),
                        end: Some(Box::new(int_expr(3))),
                        inclusive: false,
                    }),
                },
                kind: TypedRangeIteratorKind::Exclusive,
                has_end: true,
                inclusive: false,
            }),
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
        patterns: vec![TypedSwitchPattern::Expr(int_expr(value))],
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
        patterns: vec![TypedSwitchPattern::Range {
            start: Box::new(int_expr(start)),
            end: Box::new(int_expr(end)),
            inclusive,
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
