use crate::{
    ConstCommonEnv, ConstError, ConstEvalBudget, ConstValue, EarlyConstEnv, EmptyEnv,
    ResolvedConstEnv, eval_early_const_bool_expr, eval_early_const_expr, eval_early_const_int_expr,
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

#[test]
fn const_eval_budget_limits_steps_and_resets_between_outer_sessions() {
    let span = Span::new(4, 8);
    let mut budget = ConstEvalBudget::new(2, 4);

    budget.begin_session();
    assert!(budget.consume_step(span).is_ok());
    assert!(budget.consume_step(span).is_ok());
    let error = budget.consume_step(span).expect_err("third step must fail");
    assert_eq!(error.span, span);
    assert!(error.message.contains("2 step limit"), "{}", error.message);
    budget.end_session();

    budget.begin_session();
    assert!(budget.consume_step(span).is_ok());
    budget.end_session();
}

#[test]
fn const_eval_budget_limits_nested_calls_and_releases_depth() {
    let span = Span::new(9, 12);
    let mut budget = ConstEvalBudget::new(8, 2);
    budget.begin_session();

    assert!(budget.enter_call(span).is_ok());
    assert!(budget.enter_call(span).is_ok());
    let error = budget
        .enter_call(span)
        .expect_err("third nested call must fail");
    assert_eq!(error.span, span);
    assert!(
        error.message.contains("2 call depth limit"),
        "{}",
        error.message
    );
    budget.leave_call();
    assert!(budget.enter_call(span).is_ok());

    budget.end_session();
}
