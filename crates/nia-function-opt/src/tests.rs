use super::passes::*;
use super::*;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionCallee, FunctionExpr,
    FunctionExprKind, FunctionLocalKind, FunctionOp, FunctionScope, FunctionScopeId,
    FunctionTerminator, FunctionTryKind, validate_function_body,
};
use nia_ids::LocalId;
use nia_opt::NiaOptimizationLevel;
use nia_span::Span;
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
    nia_ids::InternedTyId::new(
        nia_ids::TyInternerId::for_module(nia_ids::ModuleId(0)),
        nia_ids::TyInternerIndex::from_interner_index(0),
    )
}
