use super::passes::*;
use super::*;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionCallee, FunctionExpr,
    FunctionExprKind, FunctionLocalKind, FunctionOp, FunctionScope, FunctionScopeId,
    FunctionTerminator, FunctionTryKind, LocalName, validate_function_body,
};
use nia_ids::LocalId;
use nia_opt::NiaOptimizationLevel;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use std::collections::HashSet;

mod cfg;
mod copy_and_constants;
mod dead_stores_and_bindings;
mod expr_cleanup;
mod pipeline;
mod scope_boundaries;

fn test_body(blocks: Vec<FunctionBlock>) -> FunctionBody {
    test_body_with_scopes(
        vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span: Span::default(),
        }],
        blocks,
    )
}

fn test_body_with_scopes(scopes: Vec<FunctionScope>, blocks: Vec<FunctionBlock>) -> FunctionBody {
    FunctionBody {
        span: Span::default(),
        locals: Vec::new(),
        scopes,
        blocks,
        entry: FunctionBlockId(0),
        ty: test_ty(),
    }
}

fn test_ty() -> nia_ids::InternedTyId {
    test_type_store()
        .append_for_module(nia_ids::ModuleId(0))
        .error()
}

fn test_other_ty() -> nia_ids::InternedTyId {
    test_type_store()
        .append_for_module(nia_ids::ModuleId(0))
        .primitive(nia_ty::PrimitiveTy::I8)
}

fn test_type_store() -> &'static nia_ty::TypeStore {
    static TYPE_STORE: std::sync::OnceLock<nia_ty::TypeStore> = std::sync::OnceLock::new();
    TYPE_STORE.get_or_init(nia_ty::TypeStore::new)
}

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn local_name(text: &str) -> LocalName {
    LocalName::named(sym(text))
}
