// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_function_block(
    block: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if block.stmts.is_empty() {
        return eval_function_block_without_scope(block, env);
    }
    env.push_const_scope(block.span)?;
    // Scope restoration must not depend on whether evaluation returns a value,
    // transfers control, or reports an error. Function frames reuse the same
    // environment across nested blocks, so leaking this scope would corrupt all
    // subsequent local lookup in the caller.
    let result = eval_function_block_without_scope(block, env);
    env.pop_const_scope();
    result
}

pub(super) fn eval_resolved_function_block(
    block: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if block.is_empty() {
        return eval_resolved_function_block_without_scope(block, env);
    }
    env.push_const_scope(block.span())?;
    // Keep the resolved evaluator's cleanup boundary identical to the early
    // evaluator above. In particular, `?` must not bypass `pop_const_scope`.
    let result = eval_resolved_function_block_without_scope(block, env);
    env.pop_const_scope();
    result
}

fn eval_resolved_function_block_without_scope(
    block: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for stmt in block.stmts() {
        match eval_resolved_function_stmt(stmt, env)? {
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => return Ok(flow),
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => {}
        }
    }
    block.tail().map_or(Ok(ConstEvalFlow::Void), |tail| {
        eval_resolved_function_tail_expr(tail, env)
    })
}

pub(super) fn eval_resolved_function_tail_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    env.prepare_resolved_function_result(expr)?;
    eval_resolved_const_expr_flow(expr, env)
}

fn eval_function_block_without_scope(
    block: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for stmt in &block.stmts {
        match eval_function_stmt(stmt, env)? {
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => return Ok(flow),
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => {}
        }
    }
    block
        .tail
        .as_deref()
        .map_or(Ok(ConstEvalFlow::Void), |tail| {
            eval_function_tail_expr(tail, env)
        })
}

pub(super) fn eval_function_tail_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    eval_const_expr_flow(expr, env)
}

pub(super) fn eval_function_stmt(
    stmt: &EarlyConstStmt,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    env.consume_const_eval_step(stmt.span)?;
    match &stmt.kind {
        EarlyConstStmtKind::Binding(binding) => match eval_const_expr_flow(&binding.value, env)? {
            ConstEvalFlow::Value(value) => {
                env.bind_function_local(stmt.span, binding, value)?;
                Ok(ConstEvalFlow::Void)
            }
            ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
            ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                span: stmt.span,
                message: "const binding value cannot contain loop control flow".to_string(),
            }),
            ConstEvalFlow::Void => Err(ConstError {
                span: stmt.span,
                message: "const function binding requires a value".to_string(),
            }),
        },
        EarlyConstStmtKind::PatternBinding(binding) => {
            match eval_const_expr_flow(&binding.value, env)? {
                ConstEvalFlow::Value(value) => {
                    let mut bindings = Vec::new();
                    if !patterns::early_pattern_matches(
                        &value,
                        &binding.pattern,
                        env,
                        &mut bindings,
                    )? {
                        return Err(ConstError {
                            span: binding.span,
                            message: "const binding pattern did not match its initializer"
                                .to_string(),
                        });
                    }
                    bindings.iter().try_for_each(|value| {
                        patterns::bind_function_pattern_value(value, binding, env)
                    })?;
                    Ok(ConstEvalFlow::Void)
                }
                ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
                ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
                ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                    span: stmt.span,
                    message: "const binding value cannot contain loop control flow".to_string(),
                }),
                ConstEvalFlow::Void => Err(ConstError {
                    span: stmt.span,
                    message: "const function binding requires a value".to_string(),
                }),
            }
        }
        EarlyConstStmtKind::Expr(expr) => match eval_const_expr_flow(expr, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => Ok(ConstEvalFlow::Void),
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => Ok(flow),
        },
        EarlyConstStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ConstError {
                    span: stmt.span,
                    message: "const function must return a value".to_string(),
                });
            };
            match eval_const_expr_flow(value, env)? {
                ConstEvalFlow::Value(value)
                | ConstEvalFlow::Return(value)
                | ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Return(value)),
                ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                    span: stmt.span,
                    message: "const return value cannot contain loop control flow".to_string(),
                }),
                ConstEvalFlow::Void => Err(ConstError {
                    span: stmt.span,
                    message: "const function must return a value".to_string(),
                }),
            }
        }
        EarlyConstStmtKind::Break => Ok(ConstEvalFlow::Break),
        EarlyConstStmtKind::Continue => Ok(ConstEvalFlow::Continue),
        EarlyConstStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => control_flow::eval_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        EarlyConstStmtKind::ForIn(for_in) => control_flow::eval_for_in_stmt(stmt.span, for_in, env),
        EarlyConstStmtKind::While { cond, body } => {
            control_flow::eval_while_stmt(stmt.span, cond, body, env)
        }
        EarlyConstStmtKind::Loop { body } => control_flow::eval_loop_stmt(stmt.span, body, env),
    }
}

pub(super) fn eval_resolved_function_stmt(
    stmt: &ResolvedConstStmt,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    env.consume_const_eval_step(stmt.span())?;
    match stmt.kind() {
        ResolvedConstStmtKind::Binding(binding) => {
            env.prepare_resolved_binding(binding)?;
            match eval_resolved_const_expr_flow(binding.value(), env)? {
                ConstEvalFlow::Value(value) => {
                    env.bind_resolved_function_local(stmt.span(), binding, value)?;
                    Ok(ConstEvalFlow::Void)
                }
                ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
                ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
                ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                    span: stmt.span(),
                    message: "const binding value cannot contain loop control flow".to_string(),
                }),
                ConstEvalFlow::Void => Err(ConstError {
                    span: stmt.span(),
                    message: "const function binding requires a value".to_string(),
                }),
            }
        }
        ResolvedConstStmtKind::PatternBinding(binding) => {
            env.prepare_resolved_pattern_binding(binding)?;
            match eval_resolved_const_expr_flow(binding.value(), env)? {
                ConstEvalFlow::Value(value) => {
                    let mut bindings = Vec::new();
                    if !patterns::resolved_pattern_matches(
                        &value,
                        binding.pattern(),
                        env,
                        &mut bindings,
                    )? {
                        return Err(ConstError {
                            span: binding.span(),
                            message: "const binding pattern did not match its initializer"
                                .to_string(),
                        });
                    }
                    bindings.iter().try_for_each(|value| {
                        patterns::bind_resolved_function_pattern_value(value, binding, env)
                    })?;
                    Ok(ConstEvalFlow::Void)
                }
                ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
                ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
                ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                    span: stmt.span(),
                    message: "const binding value cannot contain loop control flow".to_string(),
                }),
                ConstEvalFlow::Void => Err(ConstError {
                    span: stmt.span(),
                    message: "const function binding requires a value".to_string(),
                }),
            }
        }
        ResolvedConstStmtKind::Expr(expr) => match eval_resolved_const_expr_flow(expr, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => Ok(ConstEvalFlow::Void),
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => Ok(flow),
        },
        ResolvedConstStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ConstError {
                    span: stmt.span(),
                    message: "const function must return a value".to_string(),
                });
            };
            env.prepare_resolved_function_result(value)?;
            match eval_resolved_const_expr_flow(value, env)? {
                ConstEvalFlow::Value(value)
                | ConstEvalFlow::Return(value)
                | ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Return(value)),
                ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
                    span: stmt.span(),
                    message: "const return value cannot contain loop control flow".to_string(),
                }),
                ConstEvalFlow::Void => Err(ConstError {
                    span: stmt.span(),
                    message: "const function must return a value".to_string(),
                }),
            }
        }
        ResolvedConstStmtKind::Break => Ok(ConstEvalFlow::Break),
        ResolvedConstStmtKind::Continue => Ok(ConstEvalFlow::Continue),
        ResolvedConstStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => control_flow::eval_resolved_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        ResolvedConstStmtKind::ForIn(for_in) => {
            control_flow::eval_resolved_for_in_stmt(stmt.span(), for_in, env)
        }
        ResolvedConstStmtKind::While { cond, body } => {
            control_flow::eval_resolved_while_stmt(stmt.span(), cond, body, env)
        }
        ResolvedConstStmtKind::Loop { body } => {
            control_flow::eval_resolved_loop_stmt(stmt.span(), body, env)
        }
    }
}
