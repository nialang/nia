use crate::{
    ConstCommonEnv, ConstError, ConstValue, EarlyConstEnv, EmptyEnv, ResolvedConstEnv,
    eval_early_const_bool_expr, eval_early_const_expr, eval_early_const_int_expr,
    eval_float_literal, eval_int_literal, eval_resolved_const_int_expr,
};
use nia_const_ir::{
    ConstAssignOp, ConstNameResolution, EarlyConstAssign, EarlyConstAssignTarget, EarlyConstExpr,
    EarlyConstExprKind, EarlyConstName, EarlyConstTypeArg, ResolvedConstExpr, ResolvedConstTypeArg,
};
use nia_ids::{LayoutBuiltin, ModuleId, ModuleIdAllocator, ValueBuiltin};
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_ty::IntConst;
use std::collections::BTreeMap;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[path = "tests/lowered_collections.rs"]
mod lowered_collections;
#[path = "tests/resolution_contracts.rs"]
mod resolution_contracts;
#[path = "tests/test_environments.rs"]
mod test_environments;
