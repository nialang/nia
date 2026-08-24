// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn eval_numeric_operand_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<Result<ConstValue, ConstEvalFlow>, ConstError> {
    numeric_operand_flow(expr.span(), super::eval_const_expr_flow(expr, env)?)
}

pub(super) fn eval_binary_flow(
    span: Span,
    lhs: &EarlyConstExpr,
    op: ConstBinaryOp,
    rhs: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    macro_rules! bool_operand {
        ($expr:expr) => {
            match eval_value_or_return_flow!($expr, env) {
                ConstValue::Bool(value) => value,
                _ => {
                    return Err(ConstError {
                        span: $expr.span,
                        message: "const expression must evaluate to bool".to_string(),
                    });
                }
            }
        };
    }
    let value = match op {
        ConstBinaryOp::And => {
            let lhs = bool_operand!(lhs);
            if !lhs {
                return Ok(ConstEvalFlow::Value(ConstValue::Bool(false)));
            }
            ConstValue::Bool(bool_operand!(rhs))
        }
        ConstBinaryOp::Or => {
            let lhs = bool_operand!(lhs);
            if lhs {
                return Ok(ConstEvalFlow::Value(ConstValue::Bool(true)));
            }
            ConstValue::Bool(bool_operand!(rhs))
        }
        ConstBinaryOp::Eq | ConstBinaryOp::Ne => {
            let lhs = eval_value_or_return_flow!(lhs, env);
            let rhs = eval_value_or_return_flow!(rhs, env);
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ConstError {
                span,
                message: "const equality requires matching operand types".to_string(),
            })?;
            ConstValue::Bool(if op == ConstBinaryOp::Eq {
                equal
            } else {
                !equal
            })
        }
        ConstBinaryOp::Lt | ConstBinaryOp::Le | ConstBinaryOp::Gt | ConstBinaryOp::Ge => {
            let lhs = match eval_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            compare_values(span, lhs, op, rhs)?
        }
        _ => {
            let lhs = match eval_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            eval_numeric_binary_value(lhs, op, rhs)
                .map_err(|message| ConstError { span, message })?
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_resolved_numeric_operand_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<Result<ConstValue, ConstEvalFlow>, ConstError> {
    numeric_operand_flow(
        expr.span(),
        super::eval_resolved_const_expr_flow(expr, env)?,
    )
}

pub(super) fn eval_resolved_binary_flow(
    expr: &ResolvedConstExpr,
    lhs: &ResolvedConstExpr,
    op: ConstBinaryOp,
    rhs: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let span = expr.span();
    macro_rules! bool_operand {
        ($expr:expr) => {
            match eval_resolved_value_or_return_flow!($expr, env) {
                ConstValue::Bool(value) => value,
                _ => {
                    return Err(ConstError {
                        span: $expr.span(),
                        message: "const expression must evaluate to bool".to_string(),
                    });
                }
            }
        };
    }
    let value = match op {
        ConstBinaryOp::And => {
            let lhs = bool_operand!(lhs);
            if !lhs {
                return Ok(ConstEvalFlow::Value(ConstValue::Bool(false)));
            }
            ConstValue::Bool(bool_operand!(rhs))
        }
        ConstBinaryOp::Or => {
            let lhs = bool_operand!(lhs);
            if lhs {
                return Ok(ConstEvalFlow::Value(ConstValue::Bool(true)));
            }
            ConstValue::Bool(bool_operand!(rhs))
        }
        ConstBinaryOp::Eq | ConstBinaryOp::Ne => {
            let float_semantics = env.resolved_float_semantics(lhs);
            let lhs = eval_resolved_value_or_return_flow!(lhs, env);
            let rhs = eval_resolved_value_or_return_flow!(rhs, env);
            let equal = match (&lhs, &rhs) {
                (ConstValue::Float(lhs), ConstValue::Float(rhs)) => {
                    match eval_typed_binary_float(*lhs, ConstBinaryOp::Eq, *rhs, float_semantics)
                        .map_err(|message| ConstError { span, message })?
                    {
                        ConstValue::Bool(equal) => equal,
                        _ => unreachable!("float equality produced a non-bool const value"),
                    }
                }
                _ => values_equal(&lhs, &rhs).ok_or_else(|| ConstError {
                    span,
                    message: "const equality requires matching operand types".to_string(),
                })?,
            };
            ConstValue::Bool(if op == ConstBinaryOp::Eq {
                equal
            } else {
                !equal
            })
        }
        ConstBinaryOp::Lt | ConstBinaryOp::Le | ConstBinaryOp::Gt | ConstBinaryOp::Ge => {
            let float_semantics = env.resolved_float_semantics(lhs);
            let lhs = match eval_resolved_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_resolved_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            match (&lhs, &rhs) {
                (ConstValue::Float(lhs), ConstValue::Float(rhs)) => {
                    eval_typed_binary_float(*lhs, op, *rhs, float_semantics)
                        .map_err(|message| ConstError { span, message })?
                }
                _ => compare_values(span, lhs, op, rhs)?,
            }
        }
        _ => {
            let lhs = match eval_resolved_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_resolved_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            match (lhs, rhs) {
                (ConstValue::Int(lhs), ConstValue::Int(rhs)) => {
                    // Resolved expressions carry concrete integer width and
                    // signedness; early evaluation intentionally lacks it.
                    let value = match env.resolved_integer_semantics(expr) {
                        Some(semantics) => eval_typed_binary_int(lhs, op, rhs, semantics),
                        None => eval_binary_int(lhs, op, rhs),
                    }
                    .map_err(|message| ConstError { span, message })?;
                    ConstValue::Int(value)
                }
                (ConstValue::Float(lhs), ConstValue::Float(rhs)) => {
                    eval_typed_binary_float(lhs, op, rhs, env.resolved_float_semantics(expr))
                        .map_err(|message| ConstError { span, message })?
                }
                (lhs, rhs) => eval_numeric_binary_value(lhs, op, rhs)
                    .map_err(|message| ConstError { span, message })?,
            }
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn numeric_operand_flow(
    span: Span,
    flow: ConstEvalFlow,
) -> Result<Result<ConstValue, ConstEvalFlow>, ConstError> {
    match flow {
        ConstEvalFlow::Value(value @ (ConstValue::Int(_) | ConstValue::Float(_))) => Ok(Ok(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span,
            message: "const expression must evaluate to a numeric value".to_string(),
        }),
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => Ok(Err(flow)),
        ConstEvalFlow::Void => Err(ConstError {
            span,
            message: "const expression requires a value".to_string(),
        }),
    }
}

fn compare_values(
    span: Span,
    lhs: ConstValue,
    op: ConstBinaryOp,
    rhs: ConstValue,
) -> Result<ConstValue, ConstError> {
    match (lhs, rhs) {
        (ConstValue::Int(lhs), ConstValue::Int(rhs)) => {
            Ok(ConstValue::Bool(eval_binary_int_compare(lhs, op, rhs)))
        }
        (ConstValue::Float(lhs), ConstValue::Float(rhs)) => {
            eval_binary_float(lhs, op, rhs).map_err(|message| ConstError { span, message })
        }
        _ => Err(ConstError {
            span,
            message: "const comparison requires matching operand types".to_string(),
        }),
    }
}
