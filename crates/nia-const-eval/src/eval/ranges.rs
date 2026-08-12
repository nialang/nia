// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_range_expr_flow(
    range: &EarlyConstRange,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let start = match eval_optional_range_bound(range.start.as_deref(), env)? {
        ConstRangeBoundFlow::Value(value) => value,
        ConstRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    let end = match eval_optional_range_bound(range.end.as_deref(), env)? {
        ConstRangeBoundFlow::Value(value) => value,
        ConstRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    Ok(ConstEvalFlow::Value(ConstValue::Range(ConstRangeValue {
        start,
        end,
        inclusive: range.inclusive,
    })))
}

pub(super) fn eval_resolved_range_expr_flow(
    range: &ResolvedConstRange,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let start = match eval_resolved_optional_range_bound(range.start(), env)? {
        ConstRangeBoundFlow::Value(value) => value,
        ConstRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    let end = match eval_resolved_optional_range_bound(range.end(), env)? {
        ConstRangeBoundFlow::Value(value) => value,
        ConstRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    Ok(ConstEvalFlow::Value(ConstValue::Range(ConstRangeValue {
        start,
        end,
        inclusive: range.is_inclusive(),
    })))
}

enum ConstRangeBoundFlow {
    Value(Option<IntConst>),
    Flow(ConstEvalFlow),
}

fn eval_optional_range_bound(
    expr: Option<&EarlyConstExpr>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstRangeBoundFlow, ConstError> {
    let Some(expr) = expr else {
        return Ok(ConstRangeBoundFlow::Value(None));
    };
    eval_range_bound(expr, env)
}

fn eval_resolved_optional_range_bound(
    expr: Option<&ResolvedConstExpr>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstRangeBoundFlow, ConstError> {
    let Some(expr) = expr else {
        return Ok(ConstRangeBoundFlow::Value(None));
    };
    eval_resolved_range_bound(expr, env)
}

fn eval_range_bound(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstRangeBoundFlow, ConstError> {
    match super::eval_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => Ok(ConstRangeBoundFlow::Value(Some(value))),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: expr.span(),
            message: "const range bound must be an integer".to_string(),
        }),
        ConstEvalFlow::Return(value) => Ok(ConstRangeBoundFlow::Flow(ConstEvalFlow::Return(value))),
        ConstEvalFlow::Propagate(value) => {
            Ok(ConstRangeBoundFlow::Flow(ConstEvalFlow::Propagate(value)))
        }
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: expr.span(),
            message: "const range bound cannot contain loop control flow".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const range bound requires a value".to_string(),
        }),
    }
}

fn eval_resolved_range_bound(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstRangeBoundFlow, ConstError> {
    match super::eval_resolved_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => Ok(ConstRangeBoundFlow::Value(Some(value))),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: expr.span(),
            message: "const range bound must be an integer".to_string(),
        }),
        ConstEvalFlow::Return(value) => Ok(ConstRangeBoundFlow::Flow(ConstEvalFlow::Return(value))),
        ConstEvalFlow::Propagate(value) => {
            Ok(ConstRangeBoundFlow::Flow(ConstEvalFlow::Propagate(value)))
        }
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: expr.span(),
            message: "const range bound cannot contain loop control flow".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const range bound requires a value".to_string(),
        }),
    }
}
