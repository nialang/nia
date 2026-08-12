// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_array_literal_flow(
    elems: &EarlyConstArrayElements,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match elems {
        EarlyConstArrayElements::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_value_or_return_flow!(elem, env));
            }
            Ok(ConstEvalFlow::Value(ConstValue::Array(values)))
        }
        EarlyConstArrayElements::Repeat { value, count } => {
            let value = eval_value_or_return_flow!(value, env);
            let count_span = count.span;
            let count_value = match eval_value_or_return_flow!(count, env) {
                ConstValue::Int(value) => value,
                _ => {
                    return Err(ConstError {
                        span: count_span,
                        message: "const array repeat count must be an integer".to_string(),
                    });
                }
            };
            let count = super::int_to_array_len(count_span, count_value)?;
            let count = usize::try_from(count).map_err(|_| ConstError {
                span: count_span,
                message: "const array repeat count is too large".to_string(),
            })?;
            Ok(ConstEvalFlow::Value(ConstValue::Array(vec![value; count])))
        }
    }
}

pub(super) fn eval_resolved_array_literal_flow(
    elems: &ResolvedConstArrayElements,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match elems.kind() {
        ResolvedConstArrayElementsKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_resolved_value_or_return_flow!(elem, env));
            }
            Ok(ConstEvalFlow::Value(ConstValue::Array(values)))
        }
        ResolvedConstArrayElementsKind::Repeat { value, count } => {
            let value = eval_resolved_value_or_return_flow!(value, env);
            let count_span = count.span();
            let count_value = match eval_resolved_value_or_return_flow!(count, env) {
                ConstValue::Int(value) => value,
                _ => {
                    return Err(ConstError {
                        span: count_span,
                        message: "const array repeat count must be an integer".to_string(),
                    });
                }
            };
            let count = super::int_to_array_len(count_span, count_value)?;
            let count = usize::try_from(count).map_err(|_| ConstError {
                span: count_span,
                message: "const array repeat count is too large".to_string(),
            })?;
            Ok(ConstEvalFlow::Value(ConstValue::Array(vec![value; count])))
        }
    }
}

pub(super) fn eval_array_index_flow(
    span: Span,
    lhs: &EarlyConstExpr,
    index: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let values = match eval_value_or_return_flow!(lhs, env) {
        ConstValue::Array(values) => values,
        _ => {
            return Err(ConstError {
                span,
                message: "const index access requires an array value".to_string(),
            });
        }
    };
    let index_span = index.span;
    let index_value = match eval_value_or_return_flow!(index, env) {
        ConstValue::Int(value) => value,
        _ => {
            return Err(ConstError {
                span: index_span,
                message: "const array index must be an integer".to_string(),
            });
        }
    };
    let index = super::int_to_array_len(index_span, index_value)?;
    let index = usize::try_from(index).map_err(|_| ConstError {
        span: index_span,
        message: "const array index is too large".to_string(),
    })?;
    values
        .get(index)
        .cloned()
        .map(ConstEvalFlow::Value)
        .ok_or_else(|| ConstError {
            span,
            message: format!("const array index {index} is out of bounds"),
        })
}

pub(super) fn eval_resolved_array_index_flow(
    span: Span,
    lhs: &ResolvedConstExpr,
    index: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let values = match eval_resolved_value_or_return_flow!(lhs, env) {
        ConstValue::Array(values) => values,
        _ => {
            return Err(ConstError {
                span,
                message: "const index access requires an array value".to_string(),
            });
        }
    };
    let index_span = index.span();
    let index_value = match eval_resolved_value_or_return_flow!(index, env) {
        ConstValue::Int(value) => value,
        _ => {
            return Err(ConstError {
                span: index_span,
                message: "const array index must be an integer".to_string(),
            });
        }
    };
    let index = super::int_to_array_len(index_span, index_value)?;
    let index = usize::try_from(index).map_err(|_| ConstError {
        span: index_span,
        message: "const array index is too large".to_string(),
    })?;
    values
        .get(index)
        .cloned()
        .map(ConstEvalFlow::Value)
        .ok_or_else(|| ConstError {
            span,
            message: format!("const array index {index} is out of bounds"),
        })
}

enum SliceBoundFlow {
    Value(usize),
    Flow(ConstEvalFlow),
}

pub(super) fn eval_array_slice_flow(
    span: Span,
    lhs: &EarlyConstExpr,
    range: &EarlyConstSliceRange,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let values = match eval_value_or_return_flow!(lhs, env) {
        ConstValue::Array(values) => values,
        _ => {
            return Err(ConstError {
                span,
                message: "const slicing requires an array value".to_string(),
            });
        }
    };
    let len = values.len();
    let start = match &range.start {
        Some(start) => match eval_slice_bound_flow(start, env)? {
            SliceBoundFlow::Value(value) => value,
            SliceBoundFlow::Flow(flow) => return Ok(flow),
        },
        None => 0,
    };
    let mut end = match &range.end {
        Some(end) => match eval_slice_bound_flow(end, env)? {
            SliceBoundFlow::Value(value) => value,
            SliceBoundFlow::Flow(flow) => return Ok(flow),
        },
        None => len,
    };
    // Normalize inclusive syntax to the half-open interval used by Rust
    // slicing and validate before taking the vector subslice.
    if range.inclusive {
        end = end.checked_add(1).ok_or_else(|| ConstError {
            span,
            message: "const slice inclusive end is too large".to_string(),
        })?;
    }
    if start > end || end > len {
        return Err(ConstError {
            span,
            message: format!("const slice range {start}..{end} is out of bounds"),
        });
    }
    Ok(ConstEvalFlow::Value(ConstValue::Array(
        values[start..end].to_vec(),
    )))
}

pub(super) fn eval_resolved_array_slice_flow(
    span: Span,
    lhs: &ResolvedConstExpr,
    range: &ResolvedConstSliceRange,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let values = match eval_resolved_value_or_return_flow!(lhs, env) {
        ConstValue::Array(values) => values,
        _ => {
            return Err(ConstError {
                span,
                message: "const slicing requires an array value".to_string(),
            });
        }
    };
    let len = values.len();
    let start = match range.start() {
        Some(start) => match eval_resolved_slice_bound_flow(start, env)? {
            SliceBoundFlow::Value(value) => value,
            SliceBoundFlow::Flow(flow) => return Ok(flow),
        },
        None => 0,
    };
    let mut end = match range.end() {
        Some(end) => match eval_resolved_slice_bound_flow(end, env)? {
            SliceBoundFlow::Value(value) => value,
            SliceBoundFlow::Flow(flow) => return Ok(flow),
        },
        None => len,
    };
    if range.is_inclusive() {
        end = end.checked_add(1).ok_or_else(|| ConstError {
            span,
            message: "const slice inclusive end is too large".to_string(),
        })?;
    }
    if start > end || end > len {
        return Err(ConstError {
            span,
            message: format!("const slice range {start}..{end} is out of bounds"),
        });
    }
    Ok(ConstEvalFlow::Value(ConstValue::Array(
        values[start..end].to_vec(),
    )))
}

fn eval_slice_bound_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<SliceBoundFlow, ConstError> {
    let span = expr.span;
    let value = match super::eval_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let ConstValue::Int(value) = value else {
        return Err(ConstError {
            span,
            message: "const slice bound must be an integer".to_string(),
        });
    };
    let value = super::int_to_array_len(span, value)?;
    usize::try_from(value)
        .map(SliceBoundFlow::Value)
        .map_err(|_| ConstError {
            span,
            message: "const slice bound is too large".to_string(),
        })
}

fn eval_resolved_slice_bound_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<SliceBoundFlow, ConstError> {
    let span = expr.span();
    let value = match super::eval_resolved_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let ConstValue::Int(value) = value else {
        return Err(ConstError {
            span,
            message: "const slice bound must be an integer".to_string(),
        });
    };
    let value = super::int_to_array_len(span, value)?;
    usize::try_from(value)
        .map(SliceBoundFlow::Value)
        .map_err(|_| ConstError {
            span,
            message: "const slice bound is too large".to_string(),
        })
}
