// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) use crate::*;
pub(super) use nia_body_ir::{
    AtomicOrder, BuiltinOperator, GeneratedLocalName, LocalName, MemoryIntrinsicOp, PlaceBase,
    TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedForIn, TypedIfPattern, TypedLocal,
    TypedLocalKind, TypedLoop, TypedMatch, TypedMatchArmBody, TypedMemoryIntrinsic,
    TypedMemoryIntrinsicSource, TypedNominalPatternConstructor, TypedPattern, TypedPatternBinding,
    TypedPatternKind, TypedPlace, TypedStmt, TypedStmtKind,
};
pub(super) use nia_function_ir::{
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionForHeader, FunctionLocalKind, FunctionOp, FunctionPlaceBase,
    FunctionScope, FunctionScopeId, FunctionTerminator, FunctionTryKind,
};
pub(super) use nia_ids::{
    ClosureId, DefId, GlobalDefId, InternedTyId, LocalId, ModuleId, ModuleIdAllocator,
};
pub(super) use nia_span::Span;
pub(super) use nia_symbol::{SymbolId, stable_hash};
pub(super) use nia_ty::{BuiltinTrait, PrimitiveTy, TyKind, TypeStore};

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
    test_type_store()
        .append_for_module(test_module_id())
        .intern(TyKind::Error)
}

pub(super) fn primitive_ty(primitive: PrimitiveTy) -> InternedTyId {
    test_type_store()
        .append_for_module(test_module_id())
        .intern(TyKind::Primitive(primitive))
}

pub(super) fn lower_test_function_body(
    body: &TypedBody,
) -> Result<FunctionBody, FunctionLoweringDiagnostic> {
    lower_function_body(
        test_module_id(),
        body,
        FunctionTypeContext::for_module(test_type_store(), test_module_id()),
    )
    .map(|lowered| lowered.body)
}

fn test_type_store() -> &'static TypeStore {
    &test_type_fixture().0
}

fn test_module_id() -> ModuleId {
    test_type_fixture().1
}

fn test_type_fixture() -> &'static (TypeStore, ModuleId) {
    static FIXTURE: std::sync::OnceLock<(TypeStore, ModuleId)> = std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| {
        let mut module_ids = ModuleIdAllocator::new();
        (TypeStore::new(), module_ids.allocate())
    })
}

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn local_name(text: &str) -> LocalName {
    LocalName::named(sym(text))
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
            pattern: TypedPattern {
                ty,
                span,
                kind: TypedPatternKind::Bind {
                    local_id,
                    name: local_name("i"),
                },
            },
            item_ty: ty,
            bool_ty: ty,
            iterable_self_ty: ty,
            iterator_ty: ty,
            iter: TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Local(LocalId(10)),
            },
            body,
        })),
    }
}

pub(super) fn match_stmt_body(arms: Vec<nia_body_ir::TypedMatchArm>) -> TypedBody {
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
                kind: TypedExprKind::Match(Box::new(TypedMatch {
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

pub(super) fn match_expr_arm(value: i32, body: TypedMatchArmBody) -> nia_body_ir::TypedMatchArm {
    nia_body_ir::TypedMatchArm {
        patterns: vec![nia_body_ir::TypedPattern {
            ty: test_ty(),
            span: Span::default(),
            kind: nia_body_ir::TypedPatternKind::CheckedInt {
                value: value.into(),
            },
        }],
        body,
        span: Span::default(),
    }
}

pub(super) fn match_range_arm(
    start: i32,
    end: i32,
    inclusive: bool,
    body: TypedMatchArmBody,
) -> nia_body_ir::TypedMatchArm {
    nia_body_ir::TypedMatchArm {
        patterns: vec![nia_body_ir::TypedPattern {
            ty: test_ty(),
            span: Span::default(),
            kind: nia_body_ir::TypedPatternKind::CheckedIntRange {
                start: start.into(),
                end: end.into(),
                inclusive,
            },
        }],
        body,
        span: Span::default(),
    }
}

pub(super) fn match_default_arm(body: TypedMatchArmBody) -> nia_body_ir::TypedMatchArm {
    nia_body_ir::TypedMatchArm {
        patterns: vec![nia_body_ir::TypedPattern {
            ty: test_ty(),
            span: Span::default(),
            kind: nia_body_ir::TypedPatternKind::Wildcard,
        }],
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
