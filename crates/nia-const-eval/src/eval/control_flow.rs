// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_for_in_stmt(
    span: Span,
    for_in: &EarlyConstForIn,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match super::eval_const_expr_flow(&for_in.iter, env)? {
        ConstEvalFlow::Value(_) => {}
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => return Ok(flow),
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: for_in.iter.span(),
                message: "const for-in iterator requires a value".to_string(),
            });
        }
    }
    // Iterator execution is witness-driven. Early IR can validate evaluation
    // of the iterable, but it cannot choose the Iterator implementation.
    let _ = span;
    let _ = &for_in.pattern;
    let _ = &for_in.body;
    let _ = env;
    Err(ConstError {
        span: for_in.iter.span(),
        message: "const for-in Iterator execution is not implemented yet".to_string(),
    })
}

pub(super) fn eval_resolved_for_in_stmt(
    span: Span,
    for_in: &ResolvedConstForIn,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let iterable = match super::eval_resolved_const_expr_flow(for_in.iter(), env)? {
        ConstEvalFlow::Value(value) => value,
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => return Ok(flow),
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: for_in.iter().span(),
                message: "const for-in iterator requires a value".to_string(),
            });
        }
    };
    let mut iterator = env.resolved_for_iterator(span, for_in.iter(), iterable)?;
    for _ in 0..CONST_LOOP_LIMIT {
        env.consume_const_eval_step(span)?;
        let (next_iterator, next) = env.resolved_iterator_next(span, iterator)?;
        iterator = next_iterator;
        let ConstValue::Optional(next) = next else {
            return Err(ConstError {
                span,
                message: "const Iterator::next must return an optional value".to_string(),
            });
        };
        let Some(item) = next else {
            return Ok(ConstEvalFlow::Void);
        };
        let mut bindings = Vec::new();
        if !patterns::resolved_pattern_matches(&item, for_in.pattern(), env, &mut bindings)? {
            return Err(ConstError {
                span,
                message: "const for-in pattern did not match Iterator::Item".to_string(),
            });
        }
        // Each item receives a fresh binding scope. Always restore it before
        // interpreting the body's flow, including on binding/evaluation error.
        env.push_const_scope(span)?;
        let bind_result = bindings
            .iter()
            .try_for_each(|binding| patterns::bind_resolved_pattern_value(binding, env));
        let body_result =
            bind_result.and_then(|()| super::eval_resolved_function_block(for_in.body(), env));
        env.pop_const_scope();
        match body_result? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void | ConstEvalFlow::Continue => {}
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
        }
    }
    Err(ConstError {
        span,
        message: format!("const for-in exceeded {CONST_LOOP_LIMIT} iterations"),
    })
}

pub(super) fn eval_while_stmt(
    span: Span,
    cond: &EarlyConstExpr,
    body: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        env.consume_const_eval_step(span)?;
        let cond_value =
            match eval_condition_flow(cond, env, "const while condition must evaluate to bool")? {
                ConstConditionFlow::Value(value) => value,
                ConstConditionFlow::Flow(flow) => return Ok(flow),
            };
        if !cond_value {
            return Ok(ConstEvalFlow::Void);
        }
        match super::eval_function_block(body, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void | ConstEvalFlow::Continue => {}
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
        }
    }
    Err(ConstError {
        span,
        message: format!("const while exceeded {CONST_LOOP_LIMIT} iterations"),
    })
}

pub(super) fn eval_resolved_while_stmt(
    span: Span,
    cond: &ResolvedConstExpr,
    body: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        env.consume_const_eval_step(span)?;
        let cond_value = match eval_resolved_condition_flow(
            cond,
            env,
            "const while condition must evaluate to bool",
        )? {
            ConstConditionFlow::Value(value) => value,
            ConstConditionFlow::Flow(flow) => return Ok(flow),
        };
        if !cond_value {
            return Ok(ConstEvalFlow::Void);
        }
        match super::eval_resolved_function_block(body, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void | ConstEvalFlow::Continue => {}
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
        }
    }
    Err(ConstError {
        span,
        message: format!("const while exceeded {CONST_LOOP_LIMIT} iterations"),
    })
}

pub(super) fn eval_loop_stmt(
    span: Span,
    body: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        env.consume_const_eval_step(span)?;
        match super::eval_function_block(body, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void | ConstEvalFlow::Continue => {}
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
        }
    }
    Err(ConstError {
        span,
        message: format!("const loop exceeded {CONST_LOOP_LIMIT} iterations"),
    })
}

pub(super) fn eval_resolved_loop_stmt(
    span: Span,
    body: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        env.consume_const_eval_step(span)?;
        match super::eval_resolved_function_block(body, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void | ConstEvalFlow::Continue => {}
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
        }
    }
    Err(ConstError {
        span,
        message: format!("const loop exceeded {CONST_LOOP_LIMIT} iterations"),
    })
}

pub(super) fn eval_if_stmt(
    cond: &EarlyConstExpr,
    then_branch: &EarlyConstBlock,
    else_branch: Option<&EarlyConstBlock>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let cond_value = match eval_condition_flow(cond, env, "if condition must evaluate to bool")? {
        ConstConditionFlow::Value(value) => value,
        ConstConditionFlow::Flow(flow) => return Ok(flow),
    };
    if cond_value {
        super::eval_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ConstEvalFlow::Void), |else_branch| {
            super::eval_function_block(else_branch, env)
        })
    }
}

pub(super) fn eval_resolved_if_stmt(
    cond: &ResolvedConstExpr,
    then_branch: &ResolvedConstBlock,
    else_branch: Option<&ResolvedConstBlock>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let cond_value =
        match eval_resolved_condition_flow(cond, env, "if condition must evaluate to bool")? {
            ConstConditionFlow::Value(value) => value,
            ConstConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        super::eval_resolved_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ConstEvalFlow::Void), |else_branch| {
            super::eval_resolved_function_block(else_branch, env)
        })
    }
}

pub(super) enum ConstConditionFlow {
    Value(bool),
    Flow(ConstEvalFlow),
}

pub(super) fn eval_condition_flow(
    cond: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
    type_error: &'static str,
) -> Result<ConstConditionFlow, ConstError> {
    condition_flow(
        cond.span(),
        super::eval_const_expr_flow(cond, env)?,
        type_error,
    )
}

pub(super) fn eval_resolved_condition_flow(
    cond: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
    type_error: &'static str,
) -> Result<ConstConditionFlow, ConstError> {
    condition_flow(
        cond.span(),
        super::eval_resolved_const_expr_flow(cond, env)?,
        type_error,
    )
}

fn condition_flow(
    span: Span,
    flow: ConstEvalFlow,
    type_error: &'static str,
) -> Result<ConstConditionFlow, ConstError> {
    match flow {
        ConstEvalFlow::Value(ConstValue::Bool(value)) => Ok(ConstConditionFlow::Value(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span,
            message: type_error.to_string(),
        }),
        ConstEvalFlow::Return(value) => Ok(ConstConditionFlow::Flow(ConstEvalFlow::Return(value))),
        ConstEvalFlow::Propagate(value) => {
            Ok(ConstConditionFlow::Flow(ConstEvalFlow::Propagate(value)))
        }
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span,
            message: "const condition cannot contain loop control flow".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span,
            message: "const condition requires a value".to_string(),
        }),
    }
}
