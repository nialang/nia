// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, UnaryOp};
pub use nia_comptime_ir::{
    ComptimeBinding, ComptimeBlock, ComptimeExpr, ComptimeExprKind, ComptimeFunction,
    ComptimeNameResolution, ComptimeParam, ComptimeStmt, ComptimeStmtKind, ComptimeSwitch,
    ComptimeSwitchArm, ComptimeSwitchArmBody, ComptimeSwitchPattern,
};
use nia_ids::LayoutBuiltin;
use nia_span::Span;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Int(i128),
    Bool(bool),
    String(String),
    Struct(BTreeMap<String, ComptimeValue>),
    Optional(Option<Box<ComptimeValue>>),
    ErrorUnion(Result<Box<ComptimeValue>, Box<ComptimeValue>>),
}

enum ComptimeEvalFlow {
    Value(ComptimeValue),
    Return(ComptimeValue),
    Void,
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
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = callee;
        let _ = args;
        Err(ComptimeError {
            span,
            message: "unsupported comptime function call".to_string(),
        })
    }

    fn push_function_frame(&mut self, span: Span) -> Result<(), ComptimeError> {
        Err(ComptimeError {
            span,
            message: "comptime function calls are not available in this context".to_string(),
        })
    }

    fn pop_function_frame(&mut self) {}

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

    fn bind_switch_pattern_local(
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
    match &expr.kind {
        ComptimeExprKind::Bool(value) => Ok(ComptimeValue::Bool(*value)),
        ComptimeExprKind::Null => Ok(ComptimeValue::Optional(None)),
        ComptimeExprKind::String(literal) => literal_string(literal)
            .map(ComptimeValue::String)
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: "unsupported string literal in comptime expression".to_string(),
            }),
        ComptimeExprKind::Integer(text) => {
            eval_int_literal(text)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError {
                    span: expr.span,
                    message,
                })
        }
        ComptimeExprKind::Ident { name, resolution }
        | ComptimeExprKind::Qualified { name, resolution } => {
            if let Some(resolution) = resolution {
                env.resolve_name_resolution(expr.span, *resolution, name)
            } else {
                env.resolve_ident(expr.span, name)
            }
        }
        ComptimeExprKind::Field { lhs, name } => match eval_comptime_expr(lhs, env)? {
            ComptimeValue::Struct(fields) => {
                fields.get(name).cloned().ok_or_else(|| ComptimeError {
                    span: expr.span,
                    message: format!("unknown comptime field `{name}`"),
                })
            }
            _ => Err(ComptimeError {
                span: expr.span,
                message: "comptime field access requires a struct value".to_string(),
            }),
        },
        ComptimeExprKind::StructLiteral { fields } => eval_struct_literal(fields, env),
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
            env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)
        }
        ComptimeExprKind::Builtin {
            name,
            type_arg_span: None,
        } => env.resolve_builtin_value(expr.span, name),
        ComptimeExprKind::Call { callee, args } => {
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
                            message: format!("unsupported builtin in comptime expression: @{name}"),
                        });
                    };
                    env.resolve_layout_builtin(expr.span, builtin, *type_arg_span)
                } else {
                    env.resolve_builtin_value(expr.span, name)
                }
            } else {
                let args = args
                    .iter()
                    .map(|arg| eval_comptime_expr(arg, env))
                    .collect::<Result<Vec<_>, _>>()?;
                env.call_function(expr.span, callee, args)
            }
        }
        ComptimeExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            match eval_comptime_expr(inner, env)? {
                ComptimeValue::Int(value) => value
                    .checked_neg()
                    .map(ComptimeValue::Int)
                    .ok_or_else(|| ComptimeError {
                        span: expr.span,
                        message: "integer overflow in comptime negation".to_string(),
                    }),
                _ => Err(ComptimeError {
                    span: expr.span,
                    message: "comptime negation requires an integer".to_string(),
                }),
            }
        }
        ComptimeExprKind::Unary {
            op: UnaryOp::Not,
            expr: inner,
        } => match eval_comptime_expr(inner, env)? {
            ComptimeValue::Bool(value) => Ok(ComptimeValue::Bool(!value)),
            _ => Err(ComptimeError {
                span: expr.span,
                message: "comptime `not` requires a bool".to_string(),
            }),
        },
        ComptimeExprKind::Unary { op, .. } => Err(ComptimeError {
            span: expr.span,
            message: format!("unsupported unary operator in comptime expression: {op:?}"),
        }),
        ComptimeExprKind::OptionalSome { expr: inner } => eval_comptime_expr(inner, env)
            .map(|value| ComptimeValue::Optional(Some(Box::new(value)))),
        ComptimeExprKind::ErrorOk { expr: inner } => eval_comptime_expr(inner, env)
            .map(|value| ComptimeValue::ErrorUnion(Ok(Box::new(value)))),
        ComptimeExprKind::ErrorErr { expr: inner } => eval_comptime_expr(inner, env)
            .map(|value| ComptimeValue::ErrorUnion(Err(Box::new(value)))),
        ComptimeExprKind::Binary { lhs, op, rhs } => eval_binary(expr.span, lhs, *op, rhs, env),
        ComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => eval_comptime_if_expr(expr.span, cond, then_branch, else_branch.as_deref(), env),
        ComptimeExprKind::Switch(switch) => eval_comptime_switch_expr(switch, env),
        ComptimeExprKind::Cast { expr: inner } => eval_comptime_expr(inner, env),
        ComptimeExprKind::Block(block) => eval_value_block(block, env, "comptime expression block"),
    }
}

fn eval_comptime_if_expr(
    span: Span,
    cond: &ComptimeExpr,
    then_branch: &ComptimeBlock,
    else_branch: Option<&ComptimeExpr>,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    if eval_comptime_bool_expr(cond, env)? {
        return eval_value_block(then_branch, env, "comptime if branch");
    }
    if let Some(else_branch) = else_branch {
        eval_comptime_expr(else_branch, env)
    } else {
        Err(ComptimeError {
            span,
            message: "comptime if expression requires an else branch".to_string(),
        })
    }
}

fn eval_value_block(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
    context: &str,
) -> Result<ComptimeValue, ComptimeError> {
    match eval_function_block(block, env)? {
        ComptimeEvalFlow::Value(value) => Ok(value),
        ComptimeEvalFlow::Return(_) => Err(ComptimeError {
            span: block.span,
            message: format!("{context} cannot return from a comptime function"),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: block.span,
            message: format!("{context} requires a tail expression"),
        }),
    }
}

fn eval_comptime_switch_expr(
    switch: &ComptimeSwitch,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let target = eval_comptime_expr(&switch.target, env)?;
    let Some(matched) = matching_switch_arm(&target, switch, env)? else {
        return Err(ComptimeError {
            span: switch.span,
            message: "comptime switch expression did not match any arm".to_string(),
        });
    };
    let arm_span = matched.arm.span;
    match eval_comptime_switch_match_body(matched, env)? {
        ComptimeEvalFlow::Value(value) => Ok(value),
        ComptimeEvalFlow::Return(_) => Err(ComptimeError {
            span: arm_span,
            message: "comptime switch arm cannot return from a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: arm_span,
            message: "comptime switch arm requires a value".to_string(),
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
    env.push_function_frame(matched.arm.span)?;
    let bind_result = bind_switch_pattern_value(&binding, env);
    let result = bind_result.and_then(|()| eval_comptime_switch_arm_body(&matched.arm.body, env));
    env.pop_function_frame();
    result
}

fn bind_switch_pattern_value(
    binding: &ComptimeSwitchBinding,
    env: &mut impl ComptimeEnv,
) -> Result<(), ComptimeError> {
    env.bind_switch_pattern_local(
        binding.span,
        &binding.name,
        binding.local_id,
        binding.value.clone(),
    )
}

fn eval_struct_literal(
    fields: &[nia_comptime_ir::ComptimeFieldInit],
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let mut values = BTreeMap::new();
    for field in fields {
        if values
            .insert(field.name.clone(), eval_comptime_expr(&field.value, env)?)
            .is_some()
        {
            return Err(ComptimeError {
                span: field.span,
                message: format!("duplicate comptime struct field `{}`", field.name),
            });
        }
    }
    Ok(ComptimeValue::Struct(values))
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
    function: &ComptimeFunction,
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
    env.push_function_frame(span)?;
    for (param, value) in function.params.iter().zip(args) {
        if let Err(err) = env.bind_function_param(param.span, param, value) {
            env.pop_function_frame();
            return Err(err);
        }
    }
    let result = eval_function_block(&function.body, env).and_then(|flow| match flow {
        ComptimeEvalFlow::Value(value) | ComptimeEvalFlow::Return(value) => Ok(value),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: function.body.span,
            message: "comptime function must return a value".to_string(),
        }),
    });
    env.pop_function_frame();
    result
}

fn eval_function_block(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if block.stmts.is_empty() {
        return eval_function_block_without_scope(block, env);
    }
    env.push_function_frame(block.span)?;
    let result = eval_function_block_without_scope(block, env);
    env.pop_function_frame();
    result
}

fn eval_function_block_without_scope(
    block: &ComptimeBlock,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for stmt in &block.stmts {
        if let ComptimeEvalFlow::Return(value) = eval_function_stmt(stmt, env)? {
            return Ok(ComptimeEvalFlow::Return(value));
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
    match &expr.kind {
        ComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            if eval_comptime_bool_expr(cond, env)? {
                return eval_function_block(then_branch, env);
            }
            else_branch
                .as_deref()
                .map_or(Ok(ComptimeEvalFlow::Void), |else_branch| {
                    eval_function_tail_expr(else_branch, env)
                })
        }
        ComptimeExprKind::Switch(switch) => eval_function_switch_tail_expr(switch, env),
        ComptimeExprKind::Block(block) => eval_function_block(block, env),
        _ => eval_comptime_expr(expr, env).map(ComptimeEvalFlow::Value),
    }
}

fn eval_function_switch_tail_expr(
    switch: &ComptimeSwitch,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let target = eval_comptime_expr(&switch.target, env)?;
    let Some(matched) = matching_switch_arm(&target, switch, env)? else {
        return Err(ComptimeError {
            span: switch.span,
            message: "comptime switch expression did not match any arm".to_string(),
        });
    };
    eval_comptime_switch_match_body(matched, env)
}

fn eval_function_stmt(
    stmt: &ComptimeStmt,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match &stmt.kind {
        ComptimeStmtKind::Binding(binding) => {
            let value = eval_comptime_expr(&binding.value, env)?;
            env.bind_function_local(stmt.span, binding, value)?;
            Ok(ComptimeEvalFlow::Void)
        }
        ComptimeStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime function must return a value".to_string(),
                });
            };
            match eval_function_tail_expr(value, env)? {
                ComptimeEvalFlow::Value(value) | ComptimeEvalFlow::Return(value) => {
                    Ok(ComptimeEvalFlow::Return(value))
                }
                ComptimeEvalFlow::Void => Err(ComptimeError {
                    span: stmt.span,
                    message: "comptime function must return a value".to_string(),
                }),
            }
        }
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

fn eval_binary(
    span: Span,
    lhs: &ComptimeExpr,
    op: BinaryOp,
    rhs: &ComptimeExpr,
    env: &mut impl ComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match op {
        BinaryOp::And => {
            let lhs = eval_comptime_bool_expr(lhs, env)?;
            if !lhs {
                return Ok(ComptimeValue::Bool(false));
            }
            eval_comptime_bool_expr(rhs, env).map(ComptimeValue::Bool)
        }
        BinaryOp::Or => {
            let lhs = eval_comptime_bool_expr(lhs, env)?;
            if lhs {
                return Ok(ComptimeValue::Bool(true));
            }
            eval_comptime_bool_expr(rhs, env).map(ComptimeValue::Bool)
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            let lhs = eval_comptime_expr(lhs, env)?;
            let rhs = eval_comptime_expr(rhs, env)?;
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ComptimeError {
                span,
                message: "comptime equality requires matching operand types".to_string(),
            })?;
            Ok(ComptimeValue::Bool(if op == BinaryOp::Eq {
                equal
            } else {
                !equal
            }))
        }
        _ => {
            let lhs = eval_comptime_int_expr(lhs, env)?;
            let rhs = eval_comptime_int_expr(rhs, env)?;
            eval_binary_int(lhs, op, rhs)
                .map(ComptimeValue::Int)
                .map_err(|message| ComptimeError { span, message })
        }
    }
}

fn values_equal(lhs: &ComptimeValue, rhs: &ComptimeValue) -> Option<bool> {
    match (lhs, rhs) {
        (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Bool(lhs), ComptimeValue::Bool(rhs)) => Some(lhs == rhs),
        (ComptimeValue::String(lhs), ComptimeValue::String(rhs)) => Some(lhs == rhs),
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

        fn push_function_frame(&mut self, _span: Span) -> Result<(), ComptimeError> {
            self.scopes.push(BTreeMap::new());
            Ok(())
        }

        fn pop_function_frame(&mut self) {
            self.scopes.pop();
        }

        fn bind_switch_pattern_local(
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
