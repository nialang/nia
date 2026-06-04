// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, UnaryOp};
pub use nia_comptime_ir::{
    ComptimeArrayElements, ComptimeAssign, ComptimeAssignPathElem, ComptimeAssignTarget,
    ComptimeBinding, ComptimeBlock, ComptimeExpr, ComptimeExprKind, ComptimeForBinding,
    ComptimeForIn, ComptimeFunction, ComptimeNameResolution, ComptimeParam, ComptimeRange,
    ComptimeStmt, ComptimeStmtKind, ComptimeSwitch, ComptimeSwitchArm, ComptimeSwitchArmBody,
    ComptimeSwitchPattern, ComptimeTypeArg,
};
use nia_ids::{InternedTyId, LayoutBuiltin, ModuleId};
use nia_span::Span;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Int(i128),
    Bool(bool),
    String(String),
    Array(Vec<ComptimeValue>),
    Range(ComptimeRangeValue),
    Struct(BTreeMap<String, ComptimeValue>),
    Optional(Option<Box<ComptimeValue>>),
    ErrorUnion(Result<Box<ComptimeValue>, Box<ComptimeValue>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeRangeValue {
    pub start: Option<i128>,
    pub end: Option<i128>,
    pub inclusive: bool,
}

enum ComptimeEvalFlow {
    Value(ComptimeValue),
    Return(ComptimeValue),
    Propagate(ComptimeValue),
    Break,
    Continue,
    Void,
}

const COMPTIME_LOOP_LIMIT: usize = 100_000;

macro_rules! eval_value_or_return_flow {
    ($expr:expr, $env:expr) => {
        match eval_comptime_expr_flow($expr, $env)? {
            ComptimeEvalFlow::Value(value) => value,
            flow @ (ComptimeEvalFlow::Return(_)
            | ComptimeEvalFlow::Propagate(_)
            | ComptimeEvalFlow::Break
            | ComptimeEvalFlow::Continue) => {
                return Ok(flow);
            }
            ComptimeEvalFlow::Void => {
                return Err(ComptimeError {
                    span: $expr.span,
                    message: "comptime expression requires a value".to_string(),
                });
            }
        }
    };
}

struct ComptimeSwitchMatch<'a> {
    arm: &'a ComptimeSwitchArm,
    binding: Option<ComptimeSwitchBinding>,
}

struct ComptimeSwitchBinding {
    span: Span,
    name: String,
    local_id: Option<nia_ids::LocalId>,
    value: ComptimeValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeError {
    pub span: Span,
    pub message: String,
}

pub trait ComptimeEnv {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_name_resolution(
        &mut self,
        span: Span,
        resolution: ComptimeNameResolution,
        name: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = resolution;
        self.resolve_ident(span, name)
    }

    fn resolve_builtin_value(
        &mut self,
        span: Span,
        name: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: format!("unsupported builtin value in comptime expression: @{name}"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg_span: Span,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn call_function(
        &mut self,
        span: Span,
        callee: &ComptimeExpr,
        type_args: &[ComptimeTypeArg],
        arg_exprs: &[ComptimeExpr],
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = callee;
        let _ = type_args;
        let _ = arg_exprs;
        let _ = args;
        Err(ComptimeError {
            span,
            message: "unsupported comptime function call".to_string(),
        })
    }

    fn push_comptime_scope(&mut self, span: Span) -> Result<(), ComptimeError> {
        Err(ComptimeError {
            span,
            message: "comptime local scopes are not available in this context".to_string(),
        })
    }

    fn pop_comptime_scope(&mut self) {}

    fn push_function_frame(&mut self, span: Span) -> Result<(), ComptimeError> {
        self.push_comptime_scope(span)
    }

    fn pop_function_frame(&mut self) {
        self.pop_comptime_scope();
    }

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &ComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = param;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function parameters are not available in this context".to_string(),
        })
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        substitutions: Vec<(String, InternedTyId)>,
    ) -> Result<(), ComptimeError> {
        let _ = span;
        let _ = module_id;
        let _ = substitutions;
        Ok(())
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &ComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = binding;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function locals are not available in this context".to_string(),
        })
    }

    fn assign_local(
        &mut self,
        span: Span,
        target: &ComptimeAssignTarget,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = target;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime assignment is not available in this context".to_string(),
        })
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        local_id: Option<nia_ids::LocalId>,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = name;
        let _ = local_id;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime switch pattern locals are not available in this context".to_string(),
        })
    }
}

#[derive(Default)]
pub struct EmptyEnv;

impl ComptimeEnv for EmptyEnv {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: format!("unknown comptime value `{name}`"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg_span: Span,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in this comptime context".to_string(),
        })
    }
}

pub fn eval_comptime_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value) => Ok(value),
        ComptimeEvalFlow::Return(_) => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression cannot return from a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Propagate(_) => Err(ComptimeError {
            span: expr.span,
            message: "comptime `.?` propagation requires a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span,
            message: "comptime loop control flow requires an enclosing loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression requires a value".to_string(),
        }),
    }
}

fn eval_comptime_expr_flow(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let value =
        match &expr.kind {
            ComptimeExprKind::Bool(value) => ComptimeValue::Bool(*value),
            ComptimeExprKind::Null => ComptimeValue::Optional(None),
            ComptimeExprKind::String(literal) => literal_string(literal)
                .map(ComptimeValue::String)
                .ok_or_else(|| ComptimeError {
                    span: expr.span,
                    message: "unsupported string literal in comptime expression".to_string(),
                })?,
            ComptimeExprKind::Integer(text) => eval_int_literal(text)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError {
                    span: expr.span,
                    message,
                })?,
            ComptimeExprKind::Ident { name, resolution }
            | ComptimeExprKind::Qualified { name, resolution } => {
                if let Some(resolution) = resolution {
                    env.resolve_name_resolution(expr.span, *resolution, name)?
                } else {
                    env.resolve_ident(expr.span, name)?
                }
            }
            ComptimeExprKind::Field { lhs, name } => match eval_value_or_return_flow!(lhs, env) {
                ComptimeValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ComptimeError {
                        span: expr.span,
                        message: format!("unknown comptime field `{name}`"),
                    })?
                }
                _ => {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: "comptime field access requires a struct value".to_string(),
                    });
                }
            },
            ComptimeExprKind::Index { lhs, index } => {
                return eval_array_index_flow(expr.span, lhs, index, env);
            }
            ComptimeExprKind::ArrayLiteral { elems, .. } => {
                return eval_array_literal_flow(elems, env);
            }
            ComptimeExprKind::StructLiteral { fields, .. } => {
                return eval_struct_literal_flow(fields, env);
            }
            ComptimeExprKind::Builtin {
                name,
                type_arg_span: Some(type_arg_span),
            } => {
                let Some(builtin) = LayoutBuiltin::from_name(name) else {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: format!("unsupported builtin in comptime expression: @{name}"),
                    });
                };
                env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)?
            }
            ComptimeExprKind::Builtin {
                name,
                type_arg_span: None,
            } => env.resolve_builtin_value(expr.span, name)?,
            ComptimeExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                if let ComptimeExprKind::Builtin {
                    name,
                    type_arg_span,
                } = &callee.kind
                {
                    if !args.is_empty() {
                        return Err(ComptimeError {
                            span: expr.span,
                            message: format!(
                                "unsupported builtin call in comptime expression: @{name}"
                            ),
                        });
                    }
                    if let Some(type_arg_span) = type_arg_span {
                        let Some(builtin) = LayoutBuiltin::from_name(name) else {
                            return Err(ComptimeError {
                                span: expr.span,
                                message: format!(
                                    "unsupported builtin in comptime expression: @{name}"
                                ),
                            });
                        };
                        env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)?
                    } else {
                        env.resolve_builtin_value(expr.span, name)?
                    }
                } else {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(eval_value_or_return_flow!(arg, env));
                    }
                    env.call_function(expr.span, callee, type_args, args, values)?
                }
            }
            ComptimeExprKind::Unary {
                op: UnaryOp::Neg,
                expr: inner,
            } => match eval_value_or_return_flow!(inner, env) {
                ComptimeValue::Int(value) => value
                    .checked_neg()
                    .map(ComptimeValue::Int)
                    .ok_or_else(|| ComptimeError {
                        span: expr.span,
                        message: "integer overflow in comptime negation".to_string(),
                    })?,
                _ => {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: "comptime negation requires an integer".to_string(),
                    });
                }
            },
            ComptimeExprKind::Unary {
                op: UnaryOp::Not,
                expr: inner,
            } => match eval_value_or_return_flow!(inner, env) {
                ComptimeValue::Bool(value) => ComptimeValue::Bool(!value),
                _ => {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: "comptime `not` requires a bool".to_string(),
                    });
                }
            },
            ComptimeExprKind::Unary { op, .. } => {
                return Err(ComptimeError {
                    span: expr.span,
                    message: format!("unsupported unary operator in comptime expression: {op:?}"),
                });
            }
            ComptimeExprKind::OptionalSome { expr: inner } => {
                ComptimeValue::Optional(Some(Box::new(eval_value_or_return_flow!(inner, env))))
            }
            ComptimeExprKind::ErrorOk { expr: inner } => {
                ComptimeValue::ErrorUnion(Ok(Box::new(eval_value_or_return_flow!(inner, env))))
            }
            ComptimeExprKind::ErrorErr { expr: inner } => {
                ComptimeValue::ErrorUnion(Err(Box::new(eval_value_or_return_flow!(inner, env))))
            }
            ComptimeExprKind::Try { expr: inner } => {
                return eval_try_expr_flow(expr.span, inner, env);
            }
            ComptimeExprKind::Binary { lhs, op, rhs } => {
                return eval_binary_flow(expr.span, lhs, *op, rhs, env);
            }
            ComptimeExprKind::Assign(assign) => {
                return eval_assign_expr_flow(expr.span, assign, env);
            }
            ComptimeExprKind::Range(range) => {
                return eval_range_expr_flow(range, env);
            }
            ComptimeExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                return eval_comptime_if_expr_flow(
                    expr.span,
                    cond,
                    then_branch,
                    else_branch.as_deref(),
                    env,
                );
            }
            ComptimeExprKind::Switch(switch) => return eval_comptime_switch_expr_flow(switch, env),
            ComptimeExprKind::Cast { expr: inner } => eval_value_or_return_flow!(inner, env),
            ComptimeExprKind::Block(block) => {
                return eval_function_block(block, env);
            }
        };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_comptime_if_expr_flow(
    span: Span,
    cond: &ComptimeExpr,
    then_branch: &ComptimeBlock,
    else_branch: Option<&ComptimeExpr>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let cond_value =
        match eval_condition_flow(cond, env, "comptime expression must evaluate to bool")? {
            ComptimeConditionFlow::Value(value) => value,
            ComptimeConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        return eval_function_block(then_branch, env);
    }
    if let Some(else_branch) = else_branch {
        eval_comptime_expr_flow(else_branch, env)
    } else {
        Err(ComptimeError {
            span,
            message: "comptime if expression requires an else branch".to_string(),
        })
    }
}

fn eval_comptime_switch_expr_flow(
    switch: &ComptimeSwitch,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let target = eval_value_or_return_flow!(&switch.target, env);
    let Some(matched) = matching_switch_arm(&target, switch, env)? else {
        return Err(ComptimeError {
            span: switch.span,
            message: "comptime switch expression did not match any arm".to_string(),
        });
    };
    eval_comptime_switch_match_body(matched, env)
}

fn eval_try_expr_flow(
    span: Span,
    inner: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match eval_comptime_expr_flow(inner, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Optional(Some(value))) => {
            Ok(ComptimeEvalFlow::Value(*value))
        }
        ComptimeEvalFlow::Value(ComptimeValue::Optional(None)) => {
            Ok(ComptimeEvalFlow::Propagate(ComptimeValue::Optional(None)))
        }
        ComptimeEvalFlow::Value(ComptimeValue::ErrorUnion(Ok(value))) => {
            Ok(ComptimeEvalFlow::Value(*value))
        }
        ComptimeEvalFlow::Value(ComptimeValue::ErrorUnion(Err(value))) => Ok(
            ComptimeEvalFlow::Propagate(ComptimeValue::ErrorUnion(Err(value))),
        ),
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span,
            message: "comptime `.?` requires optional or error union operand".to_string(),
        }),
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => Ok(flow),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span,
            message: "comptime `.?` requires a value".to_string(),
        }),
    }
}

fn matching_switch_arm<'a>(
    target: &ComptimeValue,
    switch: &'a ComptimeSwitch,
    env: &mut impl ComptimeEnv,
) -> Result<Option<ComptimeSwitchMatch<'a>>, ComptimeError> {
    let mut default = None;
    for arm in &switch.arms {
        for pattern in &arm.patterns {
            match pattern {
                ComptimeSwitchPattern::Default => {
                    default = Some(arm);
                }
                ComptimeSwitchPattern::Expr(pattern) => {
                    let pattern = eval_comptime_expr(pattern, env)?;
                    if values_equal(target, &pattern).unwrap_or(false) {
                        return Ok(Some(ComptimeSwitchMatch { arm, binding: None }));
                    }
                }
                ComptimeSwitchPattern::Range {
                    start,
                    end,
                    inclusive,
                    span,
                } => {
                    if switch_range_matches(target, start, end, *inclusive, *span, env)? {
                        return Ok(Some(ComptimeSwitchMatch { arm, binding: None }));
                    }
                }
                ComptimeSwitchPattern::OptionalSome {
                    name,
                    local_id,
                    span,
                } => match target {
                    ComptimeValue::Optional(Some(value)) => {
                        return Ok(Some(ComptimeSwitchMatch {
                            arm,
                            binding: Some(ComptimeSwitchBinding {
                                span: *span,
                                name: name.clone(),
                                local_id: *local_id,
                                value: (**value).clone(),
                            }),
                        }));
                    }
                    ComptimeValue::Optional(None) => {}
                    _ => {
                        return Err(ComptimeError {
                            span: *span,
                            message: "comptime optional switch pattern requires an optional target"
                                .to_string(),
                        });
                    }
                },
                ComptimeSwitchPattern::OptionalNull { span } => match target {
                    ComptimeValue::Optional(None) => {
                        return Ok(Some(ComptimeSwitchMatch { arm, binding: None }));
                    }
                    ComptimeValue::Optional(Some(_)) => {}
                    _ => {
                        return Err(ComptimeError {
                            span: *span,
                            message: "comptime optional switch pattern requires an optional target"
                                .to_string(),
                        });
                    }
                },
                ComptimeSwitchPattern::ErrorOk {
                    name,
                    local_id,
                    span,
                } => match target {
                    ComptimeValue::ErrorUnion(Ok(value)) => {
                        return Ok(Some(ComptimeSwitchMatch {
                            arm,
                            binding: Some(ComptimeSwitchBinding {
                                span: *span,
                                name: name.clone(),
                                local_id: *local_id,
                                value: (**value).clone(),
                            }),
                        }));
                    }
                    ComptimeValue::ErrorUnion(Err(_)) => {}
                    _ => {
                        return Err(ComptimeError {
                            span: *span,
                            message: "comptime error switch pattern requires an error-union target"
                                .to_string(),
                        });
                    }
                },
                ComptimeSwitchPattern::ErrorErr {
                    name,
                    local_id,
                    span,
                } => match target {
                    ComptimeValue::ErrorUnion(Err(value)) => {
                        return Ok(Some(ComptimeSwitchMatch {
                            arm,
                            binding: Some(ComptimeSwitchBinding {
                                span: *span,
                                name: name.clone(),
                                local_id: *local_id,
                                value: (**value).clone(),
                            }),
                        }));
                    }
                    ComptimeValue::ErrorUnion(Ok(_)) => {}
                    _ => {
                        return Err(ComptimeError {
                            span: *span,
                            message: "comptime error switch pattern requires an error-union target"
                                .to_string(),
                        });
                    }
                },
            }
        }
    }
    Ok(default.map(|arm| ComptimeSwitchMatch { arm, binding: None }))
}

fn switch_range_matches(
    target: &ComptimeValue,
    start: &ComptimeExpr,
    end: &ComptimeExpr,
    inclusive: bool,
    span: Span,
    env: &mut impl ComptimeEnv,
) -> Result<bool, ComptimeError> {
    let ComptimeValue::Int(target) = target else {
        return Err(ComptimeError {
            span,
            message: "comptime switch range requires an integer target".to_string(),
        });
    };
    let start = eval_comptime_int_expr(start, env)?;
    let end = eval_comptime_int_expr(end, env)?;
    Ok(if inclusive {
        start <= *target && *target <= end
    } else {
        start <= *target && *target < end
    })
}

fn eval_comptime_switch_arm_body(
    body: &ComptimeSwitchArmBody,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match body {
        ComptimeSwitchArmBody::Expr(expr) => eval_function_tail_expr(expr, env),
        ComptimeSwitchArmBody::Stmt(stmt) => eval_function_stmt(stmt, env),
        ComptimeSwitchArmBody::Block(block) => eval_function_block(block, env),
    }
}

fn eval_comptime_switch_match_body(
    matched: ComptimeSwitchMatch<'_>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let Some(binding) = matched.binding else {
        return eval_comptime_switch_arm_body(&matched.arm.body, env);
    };
    env.push_comptime_scope(matched.arm.span)?;
    let bind_result = bind_switch_pattern_value(&binding, env);
    let result = bind_result.and_then(|()| eval_comptime_switch_arm_body(&matched.arm.body, env));
    env.pop_comptime_scope();
    result
}

fn bind_switch_pattern_value(
    binding: &ComptimeSwitchBinding,
    env: &mut impl ComptimeEnv,
) -> Result<(), ComptimeError> {
    env.bind_pattern_local(
        binding.span,
        &binding.name,
        binding.local_id,
        binding.value.clone(),
    )
}

fn eval_array_literal_flow(
    elems: &ComptimeArrayElements,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match elems {
        ComptimeArrayElements::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_value_or_return_flow!(elem, env));
            }
            Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(values)))
        }
        ComptimeArrayElements::Repeat { value, count } => {
            let value = eval_value_or_return_flow!(value, env);
            let count_span = count.span;
            let count_value = match eval_value_or_return_flow!(count, env) {
                ComptimeValue::Int(value) => value,
                _ => {
                    return Err(ComptimeError {
                        span: count_span,
                        message: "comptime array repeat count must be an integer".to_string(),
                    });
                }
            };
            let count = int_to_array_len(count_span, count_value)?;
            let count = usize::try_from(count).map_err(|_| ComptimeError {
                span: count_span,
                message: "comptime array repeat count is too large".to_string(),
            })?;
            Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(vec![
                value;
                count
            ])))
        }
    }
}

fn eval_array_index_flow(
    span: Span,
    lhs: &ComptimeExpr,
    index: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let values = match eval_value_or_return_flow!(lhs, env) {
        ComptimeValue::Array(values) => values,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime index access requires an array value".to_string(),
            });
        }
    };
    let index_span = index.span;
    let index_value = match eval_value_or_return_flow!(index, env) {
        ComptimeValue::Int(value) => value,
        _ => {
            return Err(ComptimeError {
                span: index_span,
                message: "comptime array index must be an integer".to_string(),
            });
        }
    };
    let index = int_to_array_len(index_span, index_value)?;
    let index = usize::try_from(index).map_err(|_| ComptimeError {
        span: index_span,
        message: "comptime array index is too large".to_string(),
    })?;
    values
        .get(index)
        .cloned()
        .map(ComptimeEvalFlow::Value)
        .ok_or_else(|| ComptimeError {
            span,
            message: format!("comptime array index {index} is out of bounds"),
        })
}

fn eval_struct_literal_flow(
    fields: &[nia_comptime_ir::ComptimeFieldInit],
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let mut values = BTreeMap::new();
    for field in fields {
        if values
            .insert(
                field.name.clone(),
                eval_value_or_return_flow!(&field.value, env),
            )
            .is_some()
        {
            return Err(ComptimeError {
                span: field.span,
                message: format!("duplicate comptime struct field `{}`", field.name),
            });
        }
    }
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Struct(values)))
}

pub fn eval_comptime_int_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<i128, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Int(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression must evaluate to an integer".to_string(),
        }),
    }
}

pub fn eval_comptime_bool_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<bool, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Bool(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span,
            message: "comptime expression must evaluate to bool".to_string(),
        }),
    }
}

pub fn eval_comptime_array_len_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<u64, ComptimeError> {
    int_to_array_len(expr.span, eval_comptime_int_expr(expr, env)?)
}

pub fn eval_comptime_function_call(
    span: Span,
    function_module_id: ModuleId,
    function: &ComptimeFunction,
    type_substitutions: Vec<(String, InternedTyId)>,
    args: Vec<ComptimeValue>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    if function.params.len() != args.len() {
        return Err(ComptimeError {
            span,
            message: format!(
                "comptime function argument count mismatch: expected {}, got {}",
                function.params.len(),
                args.len()
            ),
        });
    }
    env.push_comptime_scope(span)?;
    if let Err(err) = env.bind_function_context(span, function_module_id, type_substitutions) {
        env.pop_comptime_scope();
        return Err(err);
    }
    for (param, value) in function.params.iter().zip(args) {
        if let Err(err) = env.bind_function_param(param.span, param, value) {
            env.pop_comptime_scope();
            return Err(err);
        }
    }
    let result = eval_function_block(&function.body, env).and_then(|flow| match flow {
        ComptimeEvalFlow::Value(value)
        | ComptimeEvalFlow::Return(value)
        | ComptimeEvalFlow::Propagate(value) => Ok(value),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: function.body.span,
            message: "comptime loop control flow escaped its loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: function.body.span,
            message: "comptime function must return a value".to_string(),
        }),
    });
    env.pop_comptime_scope();
    result
}

fn eval_function_block(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if block.stmts.is_empty() {
        return eval_function_block_without_scope(block, env);
    }
    env.push_comptime_scope(block.span)?;
    let result = eval_function_block_without_scope(block, env);
    env.pop_comptime_scope();
    result
}

fn eval_function_block_without_scope(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for stmt in &block.stmts {
        match eval_function_stmt(stmt, env)? {
            ComptimeEvalFlow::Return(value) => return Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => return Ok(ComptimeEvalFlow::Propagate(value)),
            ComptimeEvalFlow::Break => return Ok(ComptimeEvalFlow::Break),
            ComptimeEvalFlow::Continue => return Ok(ComptimeEvalFlow::Continue),
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void => {}
        }
    }
    block
        .tail
        .as_deref()
        .map_or(Ok(ComptimeEvalFlow::Void), |tail| {
            eval_function_tail_expr(tail, env)
        })
}

fn eval_function_tail_expr(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    eval_comptime_expr_flow(expr, env)
}

fn eval_function_stmt(
    stmt: &ComptimeStmt,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match &stmt.kind {
        ComptimeStmtKind::Binding(binding) => match eval_comptime_expr_flow(&binding.value, env)? {
            ComptimeEvalFlow::Value(value) => {
                env.bind_function_local(stmt.span, binding, value)?;
                Ok(ComptimeEvalFlow::Void)
            }
            ComptimeEvalFlow::Return(value) => Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Propagate(value)),
            ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
                span: stmt.span,
                message: "comptime binding value cannot contain loop control flow".to_string(),
            }),
            ComptimeEvalFlow::Void => Err(ComptimeError {
                span: stmt.span,
                message: "comptime function binding requires a value".to_string(),
            }),
        },
        ComptimeStmtKind::Expr(expr) => match eval_comptime_expr_flow(expr, env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void => Ok(ComptimeEvalFlow::Void),
            ComptimeEvalFlow::Return(value) => Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Propagate(value)),
            ComptimeEvalFlow::Break => Ok(ComptimeEvalFlow::Break),
            ComptimeEvalFlow::Continue => Ok(ComptimeEvalFlow::Continue),
        },
        ComptimeStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime function must return a value".to_string(),
                });
            };
            match eval_comptime_expr_flow(value, env)? {
                ComptimeEvalFlow::Value(value)
                | ComptimeEvalFlow::Return(value)
                | ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Return(value)),
                ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime return value cannot contain loop control flow".to_string(),
                }),
                ComptimeEvalFlow::Void => Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime function must return a value".to_string(),
                }),
            }
        }
        ComptimeStmtKind::Break => Ok(ComptimeEvalFlow::Break),
        ComptimeStmtKind::Continue => Ok(ComptimeEvalFlow::Continue),
        ComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => eval_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        ComptimeStmtKind::ForIn(for_in) => eval_for_in_stmt(stmt.span, for_in, env),
        ComptimeStmtKind::While { cond, body } => eval_while_stmt(stmt.span, cond, body, env),
        ComptimeStmtKind::Loop { body } => eval_loop_stmt(stmt.span, body, env),
    }
}

fn eval_assign_expr_flow(
    span: Span,
    assign: &ComptimeAssign,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let value = match eval_assignment_value_flow(span, assign, env)? {
        ComptimeEvalFlow::Value(value) => value,
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => return Ok(flow),
        ComptimeEvalFlow::Void => {
            return Err(ComptimeError {
                span,
                message: "comptime assignment requires a value".to_string(),
            });
        }
    };
    let value = assign_target_writeback_value(span, &assign.lhs, value, env)?;
    env.assign_local(span, &assign.lhs, value)?;
    Ok(ComptimeEvalFlow::Void)
}

fn eval_assignment_value_flow(
    span: Span,
    assign: &ComptimeAssign,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let rhs = eval_value_or_return_flow!(&assign.rhs, env);
    if matches!(assign.op, nia_ast::AssignOp::Assign) {
        return Ok(ComptimeEvalFlow::Value(rhs));
    }
    let lhs = eval_assign_target_value(span, &assign.lhs, env)?;
    let op = assign_op_binary(assign.op).ok_or_else(|| ComptimeError {
        span,
        message: "unsupported comptime assignment operator".to_string(),
    })?;
    let (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) = (lhs, rhs) else {
        return Err(ComptimeError {
            span,
            message: "comptime compound assignment requires integer operands".to_string(),
        });
    };
    eval_binary_int(lhs, op, rhs)
        .map(ComptimeValue::Int)
        .map(ComptimeEvalFlow::Value)
        .map_err(|message| ComptimeError { span, message })
}

fn eval_assign_target_root_value(
    span: Span,
    target: &ComptimeAssignTarget,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target {
        ComptimeAssignTarget::Local {
            span: target_span,
            name,
            local_id,
            ..
        } => {
            if let Some(local_id) = local_id {
                env.resolve_name_resolution(
                    *target_span,
                    ComptimeNameResolution::Local(*local_id),
                    name,
                )
            } else {
                env.resolve_ident(span, name)
            }
        }
    }
}

fn eval_assign_target_value(
    span: Span,
    target: &ComptimeAssignTarget,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let value = eval_assign_target_root_value(span, target, env)?;
    match target {
        ComptimeAssignTarget::Local { path, .. } => eval_assign_path_value(span, value, path, env),
    }
}

fn eval_assign_path_value(
    span: Span,
    mut value: ComptimeValue,
    path: &[ComptimeAssignPathElem],
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    for elem in path {
        value = match elem {
            ComptimeAssignPathElem::Field { span, name } => match value {
                ComptimeValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ComptimeError {
                        span: *span,
                        message: format!("unknown comptime assignment field `{name}`"),
                    })?
                }
                _ => {
                    return Err(ComptimeError {
                        span: *span,
                        message: "comptime field assignment requires a struct value".to_string(),
                    });
                }
            },
            ComptimeAssignPathElem::Index {
                span: elem_span,
                index,
            } => match value {
                ComptimeValue::Array(values) => {
                    let index = eval_assign_path_index(*elem_span, index, env)?;
                    values.get(index).cloned().ok_or_else(|| ComptimeError {
                        span,
                        message: format!(
                            "comptime array assignment index {index} is out of bounds"
                        ),
                    })?
                }
                _ => {
                    return Err(ComptimeError {
                        span: *elem_span,
                        message: "comptime index assignment requires an array value".to_string(),
                    });
                }
            },
        };
    }
    Ok(value)
}

fn assign_target_writeback_value(
    span: Span,
    target: &ComptimeAssignTarget,
    value: ComptimeValue,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target {
        ComptimeAssignTarget::Local { path, .. } => {
            if path.is_empty() {
                return Ok(value);
            }
            let root = eval_assign_target_root_value(span, target, env)?;
            write_assign_path_value(span, root, path, value, env)
        }
    }
}

fn write_assign_path_value(
    span: Span,
    root: ComptimeValue,
    path: &[ComptimeAssignPathElem],
    value: ComptimeValue,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        ComptimeAssignPathElem::Field {
            span: field_span,
            name,
        } => {
            let ComptimeValue::Struct(mut fields) = root else {
                return Err(ComptimeError {
                    span: *field_span,
                    message: "comptime field assignment requires a struct value".to_string(),
                });
            };
            let current = fields.remove(name).ok_or_else(|| ComptimeError {
                span: *field_span,
                message: format!("unknown comptime assignment field `{name}`"),
            })?;
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            fields.insert(name.clone(), updated);
            Ok(ComptimeValue::Struct(fields))
        }
        ComptimeAssignPathElem::Index {
            span: index_span,
            index,
        } => {
            let ComptimeValue::Array(mut values) = root else {
                return Err(ComptimeError {
                    span: *index_span,
                    message: "comptime index assignment requires an array value".to_string(),
                });
            };
            let index = eval_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ComptimeError {
                    span,
                    message: format!("comptime array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ComptimeValue::Array(values))
        }
    }
}

fn eval_assign_path_index(
    span: Span,
    index: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<usize, ComptimeError> {
    let index_span = index.span;
    let value = match eval_comptime_expr_flow(index, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Int(value)) => value,
        ComptimeEvalFlow::Value(_) => {
            return Err(ComptimeError {
                span: index_span,
                message: "comptime array assignment index must be an integer".to_string(),
            });
        }
        ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue => {
            return Err(ComptimeError {
                span: index_span,
                message: "comptime array assignment index cannot contain control flow".to_string(),
            });
        }
        ComptimeEvalFlow::Void => {
            return Err(ComptimeError {
                span: index_span,
                message: "comptime array assignment index requires a value".to_string(),
            });
        }
    };
    let index = int_to_array_len(span, value)?;
    usize::try_from(index).map_err(|_| ComptimeError {
        span,
        message: "comptime array assignment index is too large".to_string(),
    })
}

fn assign_op_binary(op: nia_ast::AssignOp) -> Option<BinaryOp> {
    Some(match op {
        nia_ast::AssignOp::Assign => return None,
        nia_ast::AssignOp::Add => BinaryOp::Add,
        nia_ast::AssignOp::Sub => BinaryOp::Sub,
        nia_ast::AssignOp::Shl => BinaryOp::Shl,
        nia_ast::AssignOp::Shr => BinaryOp::Shr,
        nia_ast::AssignOp::Mul => BinaryOp::Mul,
        nia_ast::AssignOp::Div => BinaryOp::Div,
        nia_ast::AssignOp::Rem => BinaryOp::Rem,
        nia_ast::AssignOp::BitAnd => BinaryOp::BitAnd,
        nia_ast::AssignOp::BitXor => BinaryOp::BitXor,
        nia_ast::AssignOp::BitOr => BinaryOp::BitOr,
    })
}

fn eval_range_expr_flow(
    range: &ComptimeRange,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let start = match eval_optional_range_bound(range.start.as_deref(), env)? {
        ComptimeRangeBoundFlow::Value(value) => value,
        ComptimeRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    let end = match eval_optional_range_bound(range.end.as_deref(), env)? {
        ComptimeRangeBoundFlow::Value(value) => value,
        ComptimeRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Range(
        ComptimeRangeValue {
            start,
            end,
            inclusive: range.inclusive,
        },
    )))
}

enum ComptimeRangeBoundFlow {
    Value(Option<i128>),
    Flow(ComptimeEvalFlow),
}

fn eval_optional_range_bound(
    expr: Option<&ComptimeExpr>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    let Some(expr) = expr else {
        return Ok(ComptimeRangeBoundFlow::Value(None));
    };
    eval_range_bound(expr, env)
}

fn eval_range_bound(
    expr: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Int(value)) => {
            Ok(ComptimeRangeBoundFlow::Value(Some(value)))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: expr.span,
            message: "comptime range bound must be an integer".to_string(),
        }),
        ComptimeEvalFlow::Return(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Return(value),
        )),
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span,
            message: "comptime range bound cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span,
            message: "comptime range bound requires a value".to_string(),
        }),
    }
}

fn eval_for_in_stmt(
    span: Span,
    for_in: &ComptimeForIn,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let iter = match eval_comptime_expr_flow(&for_in.iter, env)? {
        ComptimeEvalFlow::Value(value) => value,
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => return Ok(flow),
        ComptimeEvalFlow::Void => {
            return Err(ComptimeError {
                span: for_in.iter.span,
                message: "comptime for-in iterator requires a value".to_string(),
            });
        }
    };
    match iter {
        ComptimeValue::Array(values) => {
            eval_for_in_values(span, &for_in.binding, &for_in.body, values, env)
        }
        ComptimeValue::Range(range) => {
            eval_for_in_range(span, &for_in.binding, &for_in.body, &range, env)
        }
        _ => Err(ComptimeError {
            span: for_in.iter.span,
            message: "comptime for-in requires an array or range value".to_string(),
        }),
    }
    .map(|flow| match flow {
        ComptimeEvalFlow::Break => ComptimeEvalFlow::Void,
        flow => flow,
    })
}

fn eval_for_in_values(
    span: Span,
    binding: &ComptimeForBinding,
    body: &ComptimeBlock,
    values: Vec<ComptimeValue>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for value in values {
        match eval_for_in_iteration(span, binding, body, value, env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void | ComptimeEvalFlow::Continue => {}
            flow @ (ComptimeEvalFlow::Break
            | ComptimeEvalFlow::Return(_)
            | ComptimeEvalFlow::Propagate(_)) => return Ok(flow),
        }
    }
    Ok(ComptimeEvalFlow::Void)
}

fn eval_for_in_range(
    span: Span,
    binding: &ComptimeForBinding,
    body: &ComptimeBlock,
    range: &ComptimeRangeValue,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let Some(mut current) = range.start else {
        return Err(ComptimeError {
            span,
            message: "comptime for-in range requires a start bound".to_string(),
        });
    };
    for _ in 0..COMPTIME_LOOP_LIMIT {
        if let Some(end) = range.end {
            let done = if range.inclusive {
                current > end
            } else {
                current >= end
            };
            if done {
                return Ok(ComptimeEvalFlow::Void);
            }
        }
        match eval_for_in_iteration(span, binding, body, ComptimeValue::Int(current), env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void | ComptimeEvalFlow::Continue => {}
            flow @ (ComptimeEvalFlow::Break
            | ComptimeEvalFlow::Return(_)
            | ComptimeEvalFlow::Propagate(_)) => return Ok(flow),
        }
        current = current.checked_add(1).ok_or_else(|| ComptimeError {
            span,
            message: "integer overflow in comptime for-in range".to_string(),
        })?;
    }
    Err(ComptimeError {
        span,
        message: format!("comptime for-in range exceeded {COMPTIME_LOOP_LIMIT} iterations"),
    })
}

fn eval_for_in_iteration(
    span: Span,
    binding: &ComptimeForBinding,
    body: &ComptimeBlock,
    value: ComptimeValue,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    env.push_comptime_scope(span)?;
    let bind_result = env.bind_pattern_local(binding.span, &binding.name, binding.local_id, value);
    let result = bind_result.and_then(|()| eval_function_block(body, env));
    env.pop_comptime_scope();
    result
}

fn eval_while_stmt(
    span: Span,
    cond: &ComptimeExpr,
    body: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for _ in 0..COMPTIME_LOOP_LIMIT {
        let cond_value =
            match eval_condition_flow(cond, env, "comptime while condition must evaluate to bool")?
            {
                ComptimeConditionFlow::Value(value) => value,
                ComptimeConditionFlow::Flow(flow) => return Ok(flow),
            };
        if !cond_value {
            return Ok(ComptimeEvalFlow::Void);
        }
        match eval_function_block(body, env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void | ComptimeEvalFlow::Continue => {}
            ComptimeEvalFlow::Break => return Ok(ComptimeEvalFlow::Void),
            ComptimeEvalFlow::Return(value) => return Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => return Ok(ComptimeEvalFlow::Propagate(value)),
        }
    }
    Err(ComptimeError {
        span,
        message: format!("comptime while exceeded {COMPTIME_LOOP_LIMIT} iterations"),
    })
}

fn eval_loop_stmt(
    span: Span,
    body: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for _ in 0..COMPTIME_LOOP_LIMIT {
        match eval_function_block(body, env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void | ComptimeEvalFlow::Continue => {}
            ComptimeEvalFlow::Break => return Ok(ComptimeEvalFlow::Void),
            ComptimeEvalFlow::Return(value) => return Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => return Ok(ComptimeEvalFlow::Propagate(value)),
        }
    }
    Err(ComptimeError {
        span,
        message: format!("comptime loop exceeded {COMPTIME_LOOP_LIMIT} iterations"),
    })
}

enum ComptimeConditionFlow {
    Value(bool),
    Flow(ComptimeEvalFlow),
}

fn eval_if_stmt(
    cond: &ComptimeExpr,
    then_branch: &ComptimeBlock,
    else_branch: Option<&ComptimeBlock>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let cond_value =
        match eval_condition_flow(cond, env, "comptime if condition must evaluate to bool")? {
            ComptimeConditionFlow::Value(value) => value,
            ComptimeConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        eval_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ComptimeEvalFlow::Void), |else_branch| {
            eval_function_block(else_branch, env)
        })
    }
}

fn eval_condition_flow(
    cond: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
    type_error: &'static str,
) -> Result<ComptimeConditionFlow, ComptimeError> {
    match eval_comptime_expr_flow(cond, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Bool(value)) => {
            Ok(ComptimeConditionFlow::Value(value))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: cond.span,
            message: type_error.to_string(),
        }),
        ComptimeEvalFlow::Return(value) => {
            Ok(ComptimeConditionFlow::Flow(ComptimeEvalFlow::Return(value)))
        }
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeConditionFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: cond.span,
            message: "comptime condition cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: cond.span,
            message: "comptime condition requires a value".to_string(),
        }),
    }
}

pub fn eval_int_literal(text: &str) -> Result<i128, String> {
    parse_int_literal(text)
}

pub fn eval_float_literal(text: &str) -> Result<f64, String> {
    let body = numeric_literal_body(text);
    body.replace('_', "")
        .parse::<f64>()
        .map_err(|_| "invalid float constant".to_string())
}

fn int_to_array_len(span: Span, value: i128) -> Result<u64, ComptimeError> {
    if value < 0 {
        return Err(ComptimeError {
            span,
            message: "array length must be non-negative".to_string(),
        });
    }
    u64::try_from(value).map_err(|_| ComptimeError {
        span,
        message: "array length is too large".to_string(),
    })
}

fn eval_binary_int(lhs: i128, op: BinaryOp, rhs: i128) -> Result<i128, String> {
    Ok(match op {
        BinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in comptime multiplication".to_string())?,
        BinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in comptime expression".to_string());
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| "integer overflow in comptime division".to_string())?
        }
        BinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in comptime expression".to_string());
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| "integer overflow in comptime remainder".to_string())?
        }
        BinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in comptime addition".to_string())?,
        BinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in comptime subtraction".to_string())?,
        BinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        BinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        BinaryOp::BitAnd => lhs & rhs,
        BinaryOp::BitXor => lhs ^ rhs,
        BinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in comptime expression: {op:?}"
            ));
        }
    })
}

fn eval_binary_flow(
    span: Span,
    lhs: &ComptimeExpr,
    op: BinaryOp,
    rhs: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    macro_rules! bool_operand {
        ($expr:expr) => {
            match eval_value_or_return_flow!($expr, env) {
                ComptimeValue::Bool(value) => value,
                _ => {
                    return Err(ComptimeError {
                        span: $expr.span,
                        message: "comptime expression must evaluate to bool".to_string(),
                    });
                }
            }
        };
    }
    macro_rules! int_operand {
        ($expr:expr) => {
            match eval_value_or_return_flow!($expr, env) {
                ComptimeValue::Int(value) => value,
                _ => {
                    return Err(ComptimeError {
                        span: $expr.span,
                        message: "comptime expression must evaluate to an integer".to_string(),
                    });
                }
            }
        };
    }
    let value = match op {
        BinaryOp::And => {
            let lhs = bool_operand!(lhs);
            if !lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(false)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        BinaryOp::Or => {
            let lhs = bool_operand!(lhs);
            if lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(true)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            let lhs = eval_value_or_return_flow!(lhs, env);
            let rhs = eval_value_or_return_flow!(rhs, env);
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ComptimeError {
                span,
                message: "comptime equality requires matching operand types".to_string(),
            })?;
            ComptimeValue::Bool(if op == BinaryOp::Eq { equal } else { !equal })
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let lhs = int_operand!(lhs);
            let rhs = int_operand!(rhs);
            ComptimeValue::Bool(eval_binary_int_compare(lhs, op, rhs))
        }
        _ => {
            let lhs = int_operand!(lhs);
            let rhs = int_operand!(rhs);
            eval_binary_int(lhs, op, rhs)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError { span, message })?
        }
    };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_binary_int_compare(lhs: i128, op: BinaryOp, rhs: i128) -> bool {
    match op {
        BinaryOp::Lt => lhs < rhs,
        BinaryOp::Le => lhs <= rhs,
        BinaryOp::Gt => lhs > rhs,
        BinaryOp::Ge => lhs >= rhs,
        _ => unreachable!("non-comparison binary operator routed to integer comparison"),
    }
}

fn values_equal(lhs: &ComptimeValue, rhs: &ComptimeValue) -> Option<bool> {
    match (lhs, rhs) {
        (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Bool(lhs), ComptimeValue::Bool(rhs)) => Some(lhs == rhs),
        (ComptimeValue::String(lhs), ComptimeValue::String(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Range(lhs), ComptimeValue::Range(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Array(lhs), ComptimeValue::Array(rhs)) => {
            if lhs.len() != rhs.len() {
                return Some(false);
            }
            lhs.iter()
                .zip(rhs)
                .try_fold(true, |_, (lhs, rhs)| values_equal(lhs, rhs))
        }
        (ComptimeValue::Optional(lhs), ComptimeValue::Optional(rhs)) => match (lhs, rhs) {
            (None, None) => Some(true),
            (Some(lhs), Some(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        (ComptimeValue::ErrorUnion(lhs), ComptimeValue::ErrorUnion(rhs)) => match (lhs, rhs) {
            (Ok(lhs), Ok(rhs)) | (Err(lhs), Err(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        _ => None,
    }
}

fn literal_string(literal: &nia_ast::StringLiteral) -> Option<String> {
    if literal.parts.len() != 1 {
        return None;
    }
    let text = literal.parts[0].as_str();
    text.strip_prefix('"')?
        .strip_suffix('"')
        .map(unescape_simple)
}

fn unescape_simple(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('0') => out.push('\0'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn checked_shift(lhs: i128, rhs: i128, is_left: bool) -> Result<i128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in comptime expression".to_string());
    };
    if rhs >= i128::BITS {
        return Err("shift count is out of range in comptime expression".to_string());
    }
    if is_left {
        lhs.checked_shl(rhs)
            .ok_or_else(|| "integer overflow in comptime left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in comptime right shift".to_string())
    }
}

fn parse_int_literal(text: &str) -> Result<i128, String> {
    let text = numeric_literal_body(text);
    let (radix, digits) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (16, rest)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (2, rest)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (8, rest)
        } else {
            (10, text)
        };
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err("invalid integer constant".to_string());
    }
    i128::from_str_radix(&digits, radix)
        .map_err(|_| "integer literal is out of range for comptime evaluation".to_string())
}

fn numeric_literal_body(text: &str) -> &str {
    let suffix_start = numeric_suffix_start(text).unwrap_or(text.len());
    &text[..suffix_start]
}

fn numeric_suffix_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let non_decimal_radix = text.starts_with("0x")
        || text.starts_with("0X")
        || text.starts_with("0b")
        || text.starts_with("0B")
        || text.starts_with("0o")
        || text.starts_with("0O");
    let mut index = if non_decimal_radix { 2 } else { 0 };
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'_'
            || if non_decimal_radix {
                digit_value(byte).is_some()
            } else {
                byte.is_ascii_digit()
            }
        {
            index += 1;
        } else {
            break;
        }
    }
    if !non_decimal_radix && index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    if !non_decimal_radix && index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
        index += 1;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    (index < bytes.len()).then_some(index)
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn eval_int_literal_ignores_type_suffix() {
        assert_eq!(eval_int_literal("42i32"), Ok(42));
        assert_eq!(eval_int_literal("0xffu8"), Ok(255));
        assert_eq!(eval_int_literal("1_024usize"), Ok(1024));
    }

    #[test]
    fn eval_float_literal_ignores_type_suffix_and_separators() {
        assert_eq!(eval_float_literal("0.0f64"), Ok(0.0));
        assert_eq!(eval_float_literal("1_024.5f32"), Ok(1024.5));
        assert_eq!(eval_float_literal("1.25e-1f64"), Ok(0.125));
    }

    #[test]
    fn evaluates_builtin_struct_field_conditions() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() bool {
    @builtin().target.os == "linux" and @builtin().target.pointer_width == 64
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let expr = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_bool_expr(&expr, &mut BuiltinEnv).unwrap();
        assert!(value);
    }

    #[test]
    fn evaluates_lowered_comptime_expr_directly() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() bool {
    @builtin().target.os == "linux"
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let ComptimeExprKind::Binary { lhs, .. } = &lowered.kind else {
            panic!("expected lowered binary expression");
        };
        let ComptimeExprKind::Field { name, .. } = &lhs.kind else {
            panic!("expected lowered field expression");
        };
        assert_eq!(name, "os");

        let value = eval_comptime_bool_expr(&lowered, &mut BuiltinEnv).unwrap();
        assert!(value);
    }

    #[test]
    fn evaluates_lowered_switch_with_string_patterns() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() usize {
    switch "linux" {
        "linux" => 8,
        "windows" => 4,
        _ => 2,
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_int_expr(&lowered, &mut EmptyEnv).unwrap();
        assert_eq!(value, 8);
    }

    #[test]
    fn evaluates_lowered_switch_with_optional_payload_patterns() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() usize {
    switch ?8 {
        ?value => value,
        null => 0,
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_int_expr(&lowered, &mut SwitchPatternEnv::default()).unwrap();
        assert_eq!(value, 8);
    }

    #[test]
    fn evaluates_lowered_switch_with_error_union_payload_patterns() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() usize {
    switch 5! {
        !value => value,
        error! => error,
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_int_expr(&lowered, &mut SwitchPatternEnv::default()).unwrap();
        assert_eq!(value, 5);
    }

    #[test]
    fn evaluates_lowered_array_literals_and_indexes() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() usize {
    [2, 4, 8][1]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_int_expr(&lowered, &mut EmptyEnv).unwrap();
        assert_eq!(value, 4);
    }

    #[test]
    fn evaluates_lowered_array_repeat_literals() {
        let (module, errors) = nia_parser::parse_module(
            r#"
fn main() bool {
    [7; 3] == [7, 7, 7]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
            panic!("expected function");
        };
        let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
        let lowered = nia_comptime_ir::lower_expr(expr).unwrap();
        let value = eval_comptime_bool_expr(&lowered, &mut EmptyEnv).unwrap();
        assert!(value);
    }

    struct BuiltinEnv;

    impl ComptimeEnv for BuiltinEnv {
        fn resolve_ident(
            &mut self,
            span: Span,
            name: &str,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: format!("unknown comptime value `{name}`"),
            })
        }

        fn resolve_builtin_value(
            &mut self,
            span: Span,
            name: &str,
        ) -> Result<ComptimeValue, ComptimeError> {
            if name != "builtin" {
                return Err(ComptimeError {
                    span,
                    message: format!("unsupported builtin @{name}"),
                });
            }
            let mut target = BTreeMap::new();
            target.insert("os".to_string(), ComptimeValue::String("linux".to_string()));
            target.insert("pointer_width".to_string(), ComptimeValue::Int(64));
            let mut builtin = BTreeMap::new();
            builtin.insert("target".to_string(), ComptimeValue::Struct(target));
            Ok(ComptimeValue::Struct(builtin))
        }

        fn resolve_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg_span: Span,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: "layout builtins are not available in this test".to_string(),
            })
        }
    }

    #[derive(Default)]
    struct SwitchPatternEnv {
        scopes: Vec<BTreeMap<String, ComptimeValue>>,
    }

    impl ComptimeEnv for SwitchPatternEnv {
        fn resolve_ident(
            &mut self,
            span: Span,
            name: &str,
        ) -> Result<ComptimeValue, ComptimeError> {
            self.scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).cloned())
                .ok_or_else(|| ComptimeError {
                    span,
                    message: format!("unknown comptime value `{name}`"),
                })
        }

        fn resolve_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg_span: Span,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: "layout builtins are not available in this test".to_string(),
            })
        }

        fn push_comptime_scope(&mut self, _span: Span) -> Result<(), ComptimeError> {
            self.scopes.push(BTreeMap::new());
            Ok(())
        }

        fn pop_comptime_scope(&mut self) {
            self.scopes.pop();
        }

        fn bind_pattern_local(
            &mut self,
            span: Span,
            name: &str,
            _local_id: Option<nia_ids::LocalId>,
            value: ComptimeValue,
        ) -> Result<(), ComptimeError> {
            let Some(scope) = self.scopes.last_mut() else {
                return Err(ComptimeError {
                    span,
                    message: "internal comptime switch pattern scope is missing".to_string(),
                });
            };
            scope.insert(name.to_string(), value);
            Ok(())
        }
    }
}
