pub(super) use crate::{
    ConstAssignOp, ConstNameResolution, EarlyConstAssign, EarlyConstAssignTarget, EarlyConstBlock,
    EarlyConstExpr, EarlyConstExprKind, EarlyConstFunction, EarlyConstGenericArg,
    EarlyConstLowerInputs, EarlyConstName, EarlyConstParam, EarlyConstTypeArg,
    ResolvedConstAssignPathElemKind, ResolvedConstAssignTargetKind, ResolvedConstExpr,
    ResolvedConstExprKind, ResolvedConstFunction, ResolvedConstGenericArg,
    ResolvedConstLowerInputs, lower_expr_early, lower_expr_early_with_context,
    lower_expr_resolved_with_context, resolve_expr,
};
pub(super) use nia_ids::{DefId, GlobalDefId, LayoutBuiltin, LocalId, ModuleIdAllocator};
pub(super) use nia_node_id::VersionedNodeKey;
use nia_node_id::{NodeChildPath, SyntaxKind};
pub(super) use nia_sema_ir::SemanticUseTable;
use nia_source::{SourceId, SourceRevision, SourceVersion};
pub(super) use nia_span::Span;
pub(super) use nia_symbol::SymbolId;
use nia_symbol::stable_hash;

pub(super) fn span() -> Span {
    Span::new(0, 1)
}

pub(super) fn other_span() -> Span {
    Span::new(2, 3)
}

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn int_expr(value: &str) -> EarlyConstExpr {
    EarlyConstExpr {
        span: span(),
        kind: EarlyConstExprKind::Integer(value.to_string()),
    }
}

fn node_key(kind: SyntaxKind, ordinal: u32) -> VersionedNodeKey {
    VersionedNodeKey::child_path(
        SourceVersion {
            id: SourceId(0),
            revision: SourceRevision::INITIAL,
        },
        kind,
        NodeChildPath::from_steps([ordinal]),
    )
}

pub(super) fn expr_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Expr, ordinal)
}

pub(super) fn stmt_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Stmt, ordinal)
}

pub(super) fn type_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Type, ordinal)
}

pub(super) fn ast_ident(name: &str) -> nia_ast::Expr {
    nia_ast::Expr {
        span: span(),
        node_key: expr_key(0),
        kind: nia_ast::ExprKind::Ident(sym(name)),
    }
}
