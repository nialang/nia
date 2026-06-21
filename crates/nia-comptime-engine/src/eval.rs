use crate::{
    ComptimeError, ComptimeRangeValue, ComptimeValue, EarlyComptimeEnv, ResolvedComptimeEnv,
    literals::{
        bytes_to_array, char_array_to_string, checked_shift, checked_shift_u128,
        comptime_error_message, decode_byte_char_literal, decode_char_literal,
        eval_byte_string_literal, eval_float_literal, eval_int_literal, eval_string_literal,
        string_to_char_array,
    },
};

use nia_comptime_ir::{
    ComptimeAssignOp, ComptimeBinaryOp, ComptimeNameResolution, ComptimeUnaryOp,
    EarlyComptimeArrayElements, EarlyComptimeAssign, EarlyComptimeAssignPathElem,
    EarlyComptimeAssignTarget, EarlyComptimeBlock, EarlyComptimeExpr, EarlyComptimeExprKind,
    EarlyComptimeForIn, EarlyComptimeFunction, EarlyComptimeName, EarlyComptimeParam,
    EarlyComptimePattern, EarlyComptimeRange, EarlyComptimeSliceRange, EarlyComptimeStmt,
    EarlyComptimeStmtKind, EarlyComptimeSwitch, EarlyComptimeSwitchArm, EarlyComptimeSwitchArmBody,
    ResolvedComptimeArrayElements, ResolvedComptimeArrayElementsKind, ResolvedComptimeAssign,
    ResolvedComptimeAssignPathElem, ResolvedComptimeAssignPathElemKind,
    ResolvedComptimeAssignTarget, ResolvedComptimeAssignTargetKind, ResolvedComptimeBlock,
    ResolvedComptimeExpr, ResolvedComptimeExprKind, ResolvedComptimeFieldInit,
    ResolvedComptimeForIn, ResolvedComptimeFunction, ResolvedComptimeParam,
    ResolvedComptimePatternKind, ResolvedComptimeRange, ResolvedComptimeSliceRange,
    ResolvedComptimeStmt, ResolvedComptimeStmtKind, ResolvedComptimeSwitch,
    ResolvedComptimeSwitchArm, ResolvedComptimeSwitchArmBody, ResolvedComptimeSwitchArmBodyKind,
};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId};
use nia_sema::{ArityCheck, NamedField, check_exact_arity, check_unique_field_set};
use nia_span::Span;
use nia_ty::IntConst;
use std::collections::BTreeMap;

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
    arm: &'a EarlyComptimeSwitchArm,
    bindings: Vec<ComptimeSwitchBinding>,
}

struct ComptimeSwitchBinding {
    span: Span,
    name: String,
    local_id: Option<nia_ids::LocalId>,
    value: ComptimeValue,
}
fn eval_comptime_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value) => Ok(value),
        ComptimeEvalFlow::Return(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression cannot return from a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Propagate(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime `.?` propagation requires a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span(),
            message: "comptime loop control flow requires an enclosing loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression requires a value".to_string(),
        }),
    }
}

pub fn eval_early_comptime_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    eval_comptime_expr(expr, env)
}

pub fn eval_resolved_comptime_expr(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    eval_resolved_comptime_expr_value(expr, env)
}

fn eval_resolved_comptime_expr_value(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match eval_resolved_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value) => Ok(value),
        ComptimeEvalFlow::Return(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression cannot return from a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Propagate(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime `.?` propagation requires a comptime function".to_string(),
        }),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span(),
            message: "comptime loop control flow requires an enclosing loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression requires a value".to_string(),
        }),
    }
}

macro_rules! eval_resolved_value_or_return_flow {
    ($expr:expr, $env:expr) => {
        match eval_resolved_comptime_expr_flow($expr, $env)? {
            ComptimeEvalFlow::Value(value) => value,
            flow @ (ComptimeEvalFlow::Return(_)
            | ComptimeEvalFlow::Propagate(_)
            | ComptimeEvalFlow::Break
            | ComptimeEvalFlow::Continue) => {
                return Ok(flow);
            }
            ComptimeEvalFlow::Void => {
                return Err(ComptimeError {
                    span: $expr.span(),
                    message: "comptime expression requires a value".to_string(),
                });
            }
        }
    };
}

fn eval_resolved_comptime_expr_flow(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let span = expr.span();
    let value = match expr.kind() {
        ResolvedComptimeExprKind::Bool(value) => ComptimeValue::Bool(*value),
        ResolvedComptimeExprKind::Null => ComptimeValue::Optional(None),
        ResolvedComptimeExprKind::String(literal) => eval_string_literal(literal)
            .map(|value| {
                ComptimeValue::Pointer(Box::new(ComptimeValue::Array(string_to_char_array(&value))))
            })
            .ok_or_else(|| ComptimeError {
                span,
                message: "unsupported string literal in comptime expression".to_string(),
            })?,
        ResolvedComptimeExprKind::ByteString(literal) => eval_byte_string_literal(literal)
            .map(|value| {
                ComptimeValue::Pointer(Box::new(ComptimeValue::Array(bytes_to_array(&value))))
            })
            .ok_or_else(|| ComptimeError {
                span,
                message: "unsupported byte string literal in comptime expression".to_string(),
            })?,
        ResolvedComptimeExprKind::Embed { path } => {
            let path = eval_string_literal(path).ok_or_else(|| ComptimeError {
                span,
                message: "invalid `@embed` path literal".to_string(),
            })?;
            env.resolve_embed(span, &path)?
        }
        ResolvedComptimeExprKind::Integer(text) => eval_int_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::from_i128(value)))
            .map_err(|message| ComptimeError { span, message })?,
        ResolvedComptimeExprKind::Float(text) => eval_float_literal(text)
            .map(ComptimeValue::Float)
            .map_err(|message| ComptimeError { span, message })?,
        ResolvedComptimeExprKind::Char(text) => decode_char_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ComptimeError {
                span,
                message: format!("invalid char literal `{text}` in comptime expression"),
            })?,
        ResolvedComptimeExprKind::ByteChar(text) => decode_byte_char_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ComptimeError {
                span,
                message: format!("invalid byte char literal `{text}` in comptime expression"),
            })?,
        ResolvedComptimeExprKind::Name(resolution) => {
            env.resolve_resolved_name(span, *resolution)?
        }
        ResolvedComptimeExprKind::Field { lhs, name } => {
            match eval_resolved_value_or_return_flow!(lhs, env) {
                ComptimeValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ComptimeError {
                        span,
                        message: format!("unknown comptime field `{name}`"),
                    })?
                }
                _ => {
                    return Err(ComptimeError {
                        span,
                        message: "comptime field access requires a struct value".to_string(),
                    });
                }
            }
        }
        ResolvedComptimeExprKind::BuiltinMethod { method, lhs } => {
            eval_builtin_method_value(span, *method, eval_resolved_value_or_return_flow!(lhs, env))?
        }
        ResolvedComptimeExprKind::Index { lhs, index } => {
            return eval_resolved_array_index_flow(span, lhs, index, env);
        }
        ResolvedComptimeExprKind::Slice { lhs, range } => {
            return eval_resolved_array_slice_flow(span, lhs, range, env);
        }
        ResolvedComptimeExprKind::ArrayLiteral { elems, .. } => {
            return eval_resolved_array_literal_flow(elems, env);
        }
        ResolvedComptimeExprKind::StructLiteral { fields, .. } => {
            return eval_resolved_struct_literal_flow(fields, env);
        }
        ResolvedComptimeExprKind::CompileError { message } => {
            let value = eval_resolved_value_or_return_flow!(message, env);
            let Some(message) = comptime_error_message(&value) else {
                return Err(ComptimeError {
                    span,
                    message: "builtin `@error` requires a comptime string message".to_string(),
                });
            };
            return Err(ComptimeError { span, message });
        }
        ResolvedComptimeExprKind::LayoutBuiltin { builtin, type_arg } => {
            env.resolve_resolved_layout_builtin(span, *builtin, type_arg)?
        }
        ResolvedComptimeExprKind::FieldOffsetBuiltin { type_arg, field } => {
            let Some(field) = eval_string_literal(field) else {
                return Err(ComptimeError {
                    span,
                    message: "invalid string literal in `@offset` field name".to_string(),
                });
            };
            env.resolve_resolved_field_offset_builtin(span, type_arg, &field)?
        }
        ResolvedComptimeExprKind::BuiltinValue(builtin) => {
            env.resolve_builtin_value(span, *builtin)?
        }
        ResolvedComptimeExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            if let ResolvedComptimeExprKind::BuiltinValue(builtin) = callee.kind() {
                if !args.is_empty() {
                    return Err(ComptimeError {
                        span,
                        message: format!(
                            "unsupported builtin call in comptime expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_builtin_value(span, *builtin)?
            } else if let ResolvedComptimeExprKind::LayoutBuiltin { builtin, type_arg } =
                callee.kind()
            {
                if !args.is_empty() {
                    return Err(ComptimeError {
                        span,
                        message: format!(
                            "unsupported builtin call in comptime expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_resolved_layout_builtin(span, *builtin, type_arg)?
            } else {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(eval_resolved_value_or_return_flow!(arg, env));
                }
                env.call_resolved_function(span, callee, type_args, args, values)?
            }
        }
        ResolvedComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Neg,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ComptimeValue::Int(value) => value
                .as_i128()
                .and_then(i128::checked_neg)
                .map(|value| ComptimeValue::Int(IntConst::from_i128(value)))
                .ok_or_else(|| ComptimeError {
                    span,
                    message: "integer overflow in comptime negation".to_string(),
                })?,
            ComptimeValue::Float(value) => ComptimeValue::Float(-value),
            _ => {
                return Err(ComptimeError {
                    span,
                    message: "comptime negation requires a numeric value".to_string(),
                });
            }
        },
        ResolvedComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Not,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ComptimeValue::Bool(value) => ComptimeValue::Bool(!value),
            _ => {
                return Err(ComptimeError {
                    span,
                    message: "comptime `not` requires a bool".to_string(),
                });
            }
        },
        ResolvedComptimeExprKind::Unary {
            op: ComptimeUnaryOp::BitNot,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ComptimeValue::Int(value) => ComptimeValue::Int(comptime_bit_not(value)),
            _ => {
                return Err(ComptimeError {
                    span,
                    message: "comptime bitwise not requires an integer".to_string(),
                });
            }
        },
        ResolvedComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Deref,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ComptimeValue::Pointer(value) => *value,
            _ => {
                return Err(ComptimeError {
                    span,
                    message: "comptime dereference requires a pointer value".to_string(),
                });
            }
        },
        ResolvedComptimeExprKind::Unary {
            op: ComptimeUnaryOp::RefReadOnly | ComptimeUnaryOp::Ref,
            expr: inner,
        } => ComptimeValue::Pointer(Box::new(eval_resolved_value_or_return_flow!(inner, env))),
        ResolvedComptimeExprKind::OptionalSome { expr: inner } => ComptimeValue::Optional(Some(
            Box::new(eval_resolved_value_or_return_flow!(inner, env)),
        )),
        ResolvedComptimeExprKind::ErrorOk { expr: inner } => ComptimeValue::ErrorUnion(Ok(
            Box::new(eval_resolved_value_or_return_flow!(inner, env)),
        )),
        ResolvedComptimeExprKind::ErrorErr { expr: inner } => ComptimeValue::ErrorUnion(Err(
            Box::new(eval_resolved_value_or_return_flow!(inner, env)),
        )),
        ResolvedComptimeExprKind::Try { expr: inner } => {
            return eval_resolved_try_expr_flow(span, inner, env);
        }
        ResolvedComptimeExprKind::Binary { lhs, op, rhs } => {
            return eval_resolved_binary_flow(span, lhs, *op, rhs, env);
        }
        ResolvedComptimeExprKind::Assign(assign) => {
            return eval_resolved_assign_expr_flow(span, assign, env);
        }
        ResolvedComptimeExprKind::Range(range) => {
            return eval_resolved_range_expr_flow(range, env);
        }
        ResolvedComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            return eval_resolved_comptime_if_expr_flow(
                span,
                cond,
                then_branch,
                else_branch.as_deref(),
                env,
            );
        }
        ResolvedComptimeExprKind::Switch(switch) => {
            return eval_resolved_comptime_switch_expr_flow(switch, env);
        }
        ResolvedComptimeExprKind::Cast { expr: inner, ty } => {
            let value = eval_resolved_value_or_return_flow!(inner, env);
            env.cast_value(span, value, *ty)?
        }
        ResolvedComptimeExprKind::Block(block) => {
            return eval_resolved_function_block(block, env);
        }
    };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_comptime_expr_flow(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let value = match &expr.kind {
        EarlyComptimeExprKind::Bool(value) => ComptimeValue::Bool(*value),
        EarlyComptimeExprKind::Null => ComptimeValue::Optional(None),
        EarlyComptimeExprKind::String(literal) => eval_string_literal(literal)
            .map(|value| {
                ComptimeValue::Pointer(Box::new(ComptimeValue::Array(string_to_char_array(&value))))
            })
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: "unsupported string literal in comptime expression".to_string(),
            })?,
        EarlyComptimeExprKind::ByteString(literal) => eval_byte_string_literal(literal)
            .map(|value| {
                ComptimeValue::Pointer(Box::new(ComptimeValue::Array(bytes_to_array(&value))))
            })
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: "unsupported byte string literal in comptime expression".to_string(),
            })?,
        EarlyComptimeExprKind::Embed { path } => {
            let path = eval_string_literal(path).ok_or_else(|| ComptimeError {
                span: expr.span,
                message: "invalid `@embed` path literal".to_string(),
            })?;
            env.resolve_embed(expr.span, &path)?
        }
        EarlyComptimeExprKind::Integer(text) => eval_int_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::from_i128(value)))
            .map_err(|message| ComptimeError {
                span: expr.span,
                message,
            })?,
        EarlyComptimeExprKind::Float(text) => eval_float_literal(text)
            .map(ComptimeValue::Float)
            .map_err(|message| ComptimeError {
                span: expr.span,
                message,
            })?,
        EarlyComptimeExprKind::Char(text) => decode_char_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: format!("invalid char literal `{text}` in comptime expression"),
            })?,
        EarlyComptimeExprKind::ByteChar(text) => decode_byte_char_literal(text)
            .map(|value| ComptimeValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ComptimeError {
                span: expr.span,
                message: format!("invalid byte char literal `{text}` in comptime expression"),
            })?,
        EarlyComptimeExprKind::Ident(name) | EarlyComptimeExprKind::Qualified(name) => {
            env.resolve_name(expr.span, name)?
        }
        EarlyComptimeExprKind::Field { lhs, name } => match eval_value_or_return_flow!(lhs, env) {
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
        EarlyComptimeExprKind::BuiltinMethod { method, lhs } => {
            eval_builtin_method_value(expr.span, *method, eval_value_or_return_flow!(lhs, env))?
        }
        EarlyComptimeExprKind::Index { lhs, index } => {
            return eval_array_index_flow(expr.span, lhs, index, env);
        }
        EarlyComptimeExprKind::Slice { lhs, range } => {
            return eval_array_slice_flow(expr.span, lhs, range, env);
        }
        EarlyComptimeExprKind::ArrayLiteral { elems, .. } => {
            return eval_array_literal_flow(elems, env);
        }
        EarlyComptimeExprKind::StructLiteral { fields, .. } => {
            return eval_struct_literal_flow(fields, env);
        }
        EarlyComptimeExprKind::CompileError { message } => {
            let value = eval_value_or_return_flow!(message, env);
            let Some(message) = comptime_error_message(&value) else {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "builtin `@error` requires a comptime string message".to_string(),
                });
            };
            return Err(ComptimeError {
                span: expr.span,
                message,
            });
        }
        EarlyComptimeExprKind::LayoutBuiltin { builtin, type_arg } => {
            env.resolve_layout_builtin(expr.span, *builtin, type_arg)?
        }
        EarlyComptimeExprKind::FieldOffsetBuiltin { type_arg, field } => {
            let Some(field) = eval_string_literal(field) else {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "invalid string literal in `@offset` field name".to_string(),
                });
            };
            env.resolve_field_offset_builtin(expr.span, type_arg, &field)?
        }
        EarlyComptimeExprKind::BuiltinValue(builtin) => {
            env.resolve_builtin_value(expr.span, *builtin)?
        }
        EarlyComptimeExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            if let EarlyComptimeExprKind::BuiltinValue(builtin) = &callee.kind {
                if !args.is_empty() {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin call in comptime expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_builtin_value(expr.span, *builtin)?
            } else if let EarlyComptimeExprKind::LayoutBuiltin { builtin, type_arg } = &callee.kind
            {
                if !args.is_empty() {
                    return Err(ComptimeError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin call in comptime expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_layout_builtin(expr.span, *builtin, type_arg)?
            } else {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(eval_value_or_return_flow!(arg, env));
                }
                env.call_function(expr.span, callee, type_args, args, values)?
            }
        }
        EarlyComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Neg,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ComptimeValue::Int(value) => value
                .as_i128()
                .and_then(i128::checked_neg)
                .map(|value| ComptimeValue::Int(IntConst::from_i128(value)))
                .ok_or_else(|| ComptimeError {
                    span: expr.span,
                    message: "integer overflow in comptime negation".to_string(),
                })?,
            ComptimeValue::Float(value) => ComptimeValue::Float(-value),
            _ => {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "comptime negation requires a numeric value".to_string(),
                });
            }
        },
        EarlyComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Not,
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
        EarlyComptimeExprKind::Unary {
            op: ComptimeUnaryOp::BitNot,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ComptimeValue::Int(value) => ComptimeValue::Int(comptime_bit_not(value)),
            _ => {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "comptime bitwise not requires an integer".to_string(),
                });
            }
        },
        EarlyComptimeExprKind::Unary {
            op: ComptimeUnaryOp::Deref,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ComptimeValue::Pointer(value) => *value,
            _ => {
                return Err(ComptimeError {
                    span: expr.span,
                    message: "comptime dereference requires a pointer value".to_string(),
                });
            }
        },
        EarlyComptimeExprKind::Unary {
            op: ComptimeUnaryOp::RefReadOnly | ComptimeUnaryOp::Ref,
            expr: inner,
        } => ComptimeValue::Pointer(Box::new(eval_value_or_return_flow!(inner, env))),
        EarlyComptimeExprKind::OptionalSome { expr: inner } => {
            ComptimeValue::Optional(Some(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyComptimeExprKind::ErrorOk { expr: inner } => {
            ComptimeValue::ErrorUnion(Ok(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyComptimeExprKind::ErrorErr { expr: inner } => {
            ComptimeValue::ErrorUnion(Err(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyComptimeExprKind::Try { expr: inner } => {
            return eval_try_expr_flow(expr.span, inner, env);
        }
        EarlyComptimeExprKind::Binary { lhs, op, rhs } => {
            return eval_binary_flow(expr.span, lhs, *op, rhs, env);
        }
        EarlyComptimeExprKind::Assign(assign) => {
            return eval_assign_expr_flow(expr.span, assign, env);
        }
        EarlyComptimeExprKind::Range(range) => {
            return eval_range_expr_flow(range, env);
        }
        EarlyComptimeExprKind::If {
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
        EarlyComptimeExprKind::Switch(switch) => {
            return eval_comptime_switch_expr_flow(switch, env);
        }
        EarlyComptimeExprKind::Cast {
            expr: inner,
            ty: Some(ty),
        } => {
            let value = eval_value_or_return_flow!(inner, env);
            env.cast_value(expr.span, value, *ty)?
        }
        EarlyComptimeExprKind::Cast {
            expr: inner,
            ty: None,
        } => eval_value_or_return_flow!(inner, env),
        EarlyComptimeExprKind::Block(block) => {
            return eval_function_block(block, env);
        }
    };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_comptime_if_expr_flow(
    span: Span,
    cond: &EarlyComptimeExpr,
    then_branch: &EarlyComptimeBlock,
    else_branch: Option<&EarlyComptimeExpr>,
    env: &mut impl EarlyComptimeEnv,
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
            message: "if expression requires an else branch".to_string(),
        })
    }
}

fn eval_builtin_method_value(
    span: Span,
    method: BuiltinTraitMethod,
    value: ComptimeValue,
) -> Result<ComptimeValue, ComptimeError> {
    match method {
        BuiltinTraitMethod::Len => eval_builtin_len_value(span, value),
        BuiltinTraitMethod::Start => eval_builtin_range_bound_value(span, value, true),
        BuiltinTraitMethod::End => eval_builtin_range_bound_value(span, value, false),
        _ => Err(ComptimeError {
            span,
            message: format!(
                "unsupported builtin trait method in comptime expression: {}",
                method.name()
            ),
        }),
    }
}

fn eval_builtin_len_value(
    span: Span,
    value: ComptimeValue,
) -> Result<ComptimeValue, ComptimeError> {
    match value {
        ComptimeValue::Array(values) => Ok(ComptimeValue::Int(IntConst::unsigned(
            u128::try_from(values.len()).map_err(|_| ComptimeError {
                span,
                message: "comptime array length is too large".to_string(),
            })?,
        ))),
        _ => Err(ComptimeError {
            span,
            message: "comptime len requires an array value".to_string(),
        }),
    }
}

fn eval_builtin_range_bound_value(
    span: Span,
    value: ComptimeValue,
    want_start: bool,
) -> Result<ComptimeValue, ComptimeError> {
    let ComptimeValue::Range(range) = value else {
        return Err(ComptimeError {
            span,
            message: "comptime range bound method requires a range value".to_string(),
        });
    };
    let bound = if want_start { range.start } else { range.end };
    let Some(bound) = bound else {
        let name = if want_start { "start" } else { "end" };
        return Err(ComptimeError {
            span,
            message: format!("comptime range does not have a {name} bound"),
        });
    };
    Ok(ComptimeValue::Int(bound))
}

fn eval_comptime_switch_expr_flow(
    switch: &EarlyComptimeSwitch,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_comptime_if_expr_flow(
    span: Span,
    cond: &ResolvedComptimeExpr,
    then_branch: &ResolvedComptimeBlock,
    else_branch: Option<&ResolvedComptimeExpr>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let cond_value =
        match eval_resolved_condition_flow(cond, env, "comptime expression must evaluate to bool")?
        {
            ComptimeConditionFlow::Value(value) => value,
            ComptimeConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        return eval_resolved_function_block(then_branch, env);
    }
    if let Some(else_branch) = else_branch {
        eval_resolved_comptime_expr_flow(else_branch, env)
    } else {
        Err(ComptimeError {
            span,
            message: "if expression requires an else branch".to_string(),
        })
    }
}

struct ResolvedComptimeSwitchMatch<'a> {
    arm: &'a ResolvedComptimeSwitchArm,
    bindings: Vec<ComptimeSwitchBinding>,
}

fn eval_resolved_comptime_switch_expr_flow(
    switch: &ResolvedComptimeSwitch,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let target = eval_resolved_value_or_return_flow!(switch.target(), env);
    let Some(matched) = matching_resolved_switch_arm(&target, switch, env)? else {
        return Err(ComptimeError {
            span: switch.span(),
            message: "comptime switch expression did not match any arm".to_string(),
        });
    };
    eval_resolved_comptime_switch_match_body(matched, env)
}

fn eval_try_expr_flow(
    span: Span,
    inner: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_try_expr_flow(
    span: Span,
    inner: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match eval_resolved_comptime_expr_flow(inner, env)? {
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
    switch: &'a EarlyComptimeSwitch,
    env: &mut impl EarlyComptimeEnv,
) -> Result<Option<ComptimeSwitchMatch<'a>>, ComptimeError> {
    let mut default = None;
    for arm in &switch.arms {
        for pattern in &arm.patterns {
            if matches!(pattern, EarlyComptimePattern::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if early_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ComptimeSwitchMatch { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ComptimeSwitchMatch {
        arm,
        bindings: Vec::new(),
    }))
}

fn matching_resolved_switch_arm<'a>(
    target: &ComptimeValue,
    switch: &'a ResolvedComptimeSwitch,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<Option<ResolvedComptimeSwitchMatch<'a>>, ComptimeError> {
    let mut default = None;
    for arm in switch.arms() {
        for pattern in arm.patterns() {
            if matches!(pattern.kind(), ResolvedComptimePatternKind::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if resolved_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ResolvedComptimeSwitchMatch { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ResolvedComptimeSwitchMatch {
        arm,
        bindings: Vec::new(),
    }))
}

fn early_pattern_matches(
    target: &ComptimeValue,
    pattern: &EarlyComptimePattern,
    env: &mut impl EarlyComptimeEnv,
    bindings: &mut Vec<ComptimeSwitchBinding>,
) -> Result<bool, ComptimeError> {
    match pattern {
        EarlyComptimePattern::Wildcard { .. } => Ok(true),
        EarlyComptimePattern::Bind {
            name,
            local_id,
            span,
        } => {
            bindings.push(ComptimeSwitchBinding {
                span: *span,
                name: name.clone(),
                local_id: *local_id,
                value: target.clone(),
            });
            Ok(true)
        }
        EarlyComptimePattern::OptionalSome { pattern, span } => match target {
            ComptimeValue::Optional(Some(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::Optional(None) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime optional switch pattern requires an optional target".to_string(),
            }),
        },
        EarlyComptimePattern::OptionalNull { span } => match target {
            ComptimeValue::Optional(None) => Ok(true),
            ComptimeValue::Optional(Some(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime null switch pattern requires an optional target".to_string(),
            }),
        },
        EarlyComptimePattern::ErrorOk { pattern, span } => match target {
            ComptimeValue::ErrorUnion(Ok(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime error-ok switch pattern requires an error union target"
                    .to_string(),
            }),
        },
        EarlyComptimePattern::ErrorErr { pattern, span } => match target {
            ComptimeValue::ErrorUnion(Err(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime error switch pattern requires an error union target".to_string(),
            }),
        },
        EarlyComptimePattern::Expr(pattern) => {
            let pattern = eval_comptime_expr(pattern, env)?;
            Ok(values_equal(target, &pattern).unwrap_or(false))
        }
        EarlyComptimePattern::Range {
            start,
            end,
            inclusive,
            span,
        } => switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn resolved_pattern_matches(
    target: &ComptimeValue,
    pattern: &nia_comptime_ir::ResolvedComptimePattern,
    env: &mut impl ResolvedComptimeEnv,
    bindings: &mut Vec<ComptimeSwitchBinding>,
) -> Result<bool, ComptimeError> {
    match pattern.kind() {
        ResolvedComptimePatternKind::Wildcard { .. } => Ok(true),
        ResolvedComptimePatternKind::Bind {
            name,
            local_id,
            span,
        } => {
            bindings.push(ComptimeSwitchBinding {
                span: *span,
                name: name.clone(),
                local_id: Some(*local_id),
                value: target.clone(),
            });
            Ok(true)
        }
        ResolvedComptimePatternKind::OptionalSome { pattern, span } => match target {
            ComptimeValue::Optional(Some(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::Optional(None) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime optional switch pattern requires an optional target".to_string(),
            }),
        },
        ResolvedComptimePatternKind::OptionalNull { span } => match target {
            ComptimeValue::Optional(None) => Ok(true),
            ComptimeValue::Optional(Some(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime null switch pattern requires an optional target".to_string(),
            }),
        },
        ResolvedComptimePatternKind::ErrorOk { pattern, span } => match target {
            ComptimeValue::ErrorUnion(Ok(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime error-ok switch pattern requires an error union target"
                    .to_string(),
            }),
        },
        ResolvedComptimePatternKind::ErrorErr { pattern, span } => match target {
            ComptimeValue::ErrorUnion(Err(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ComptimeValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ComptimeError {
                span: *span,
                message: "comptime error switch pattern requires an error union target".to_string(),
            }),
        },
        ResolvedComptimePatternKind::Expr(pattern) => {
            let pattern = eval_resolved_comptime_expr_value(pattern, env)?;
            Ok(values_equal(target, &pattern).unwrap_or(false))
        }
        ResolvedComptimePatternKind::Range {
            start,
            end,
            inclusive,
            span,
        } => resolved_switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn switch_range_matches(
    target: &ComptimeValue,
    start: &EarlyComptimeExpr,
    end: &EarlyComptimeExpr,
    inclusive: bool,
    span: Span,
    env: &mut impl EarlyComptimeEnv,
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
        eval_binary_int_compare(start, ComptimeBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ComptimeBinaryOp::Le, end)
    } else {
        eval_binary_int_compare(start, ComptimeBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ComptimeBinaryOp::Lt, end)
    })
}

fn resolved_switch_range_matches(
    target: &ComptimeValue,
    start: &ResolvedComptimeExpr,
    end: &ResolvedComptimeExpr,
    inclusive: bool,
    span: Span,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<bool, ComptimeError> {
    let ComptimeValue::Int(target) = target else {
        return Err(ComptimeError {
            span,
            message: "comptime switch range requires an integer target".to_string(),
        });
    };
    let start = eval_resolved_comptime_int_expr_inner(start, env)?;
    let end = eval_resolved_comptime_int_expr_inner(end, env)?;
    Ok(if inclusive {
        eval_binary_int_compare(start, ComptimeBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ComptimeBinaryOp::Le, end)
    } else {
        eval_binary_int_compare(start, ComptimeBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ComptimeBinaryOp::Lt, end)
    })
}

fn eval_comptime_switch_arm_body(
    body: &EarlyComptimeSwitchArmBody,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match body {
        EarlyComptimeSwitchArmBody::Expr(expr) => eval_function_tail_expr(expr, env),
        EarlyComptimeSwitchArmBody::Stmt(stmt) => eval_function_stmt(stmt, env),
        EarlyComptimeSwitchArmBody::Block(block) => eval_function_block(block, env),
    }
}

fn eval_comptime_switch_match_body(
    matched: ComptimeSwitchMatch<'_>,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if matched.bindings.is_empty() {
        return eval_comptime_switch_arm_body(&matched.arm.body, env);
    }
    env.push_comptime_scope(matched.arm.span)?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| bind_pattern_value(binding, env));
    let result = bind_result.and_then(|()| eval_comptime_switch_arm_body(&matched.arm.body, env));
    env.pop_comptime_scope();
    result
}

fn eval_resolved_comptime_switch_arm_body(
    body: &ResolvedComptimeSwitchArmBody,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match body.kind() {
        ResolvedComptimeSwitchArmBodyKind::Expr(expr) => {
            eval_resolved_function_tail_expr(expr, env)
        }
        ResolvedComptimeSwitchArmBodyKind::Stmt(stmt) => eval_resolved_function_stmt(stmt, env),
        ResolvedComptimeSwitchArmBodyKind::Block(block) => eval_resolved_function_block(block, env),
    }
}

fn eval_resolved_comptime_switch_match_body(
    matched: ResolvedComptimeSwitchMatch<'_>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if matched.bindings.is_empty() {
        return eval_resolved_comptime_switch_arm_body(matched.arm.body(), env);
    }
    env.push_comptime_scope(matched.arm.span())?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| bind_resolved_pattern_value(binding, env));
    let result =
        bind_result.and_then(|()| eval_resolved_comptime_switch_arm_body(matched.arm.body(), env));
    env.pop_comptime_scope();
    result
}

fn bind_pattern_value(
    binding: &ComptimeSwitchBinding,
    env: &mut impl EarlyComptimeEnv,
) -> Result<(), ComptimeError> {
    env.bind_pattern_local(
        binding.span,
        &binding.name,
        binding.local_id,
        binding.value.clone(),
    )
}

fn bind_resolved_pattern_value(
    binding: &ComptimeSwitchBinding,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<(), ComptimeError> {
    let local_id = binding
        .local_id
        .expect("resolved comptime switch pattern must have a local id");
    env.bind_resolved_pattern_local(binding.span, &binding.name, local_id, binding.value.clone())
}

fn eval_array_literal_flow(
    elems: &EarlyComptimeArrayElements,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match elems {
        EarlyComptimeArrayElements::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_value_or_return_flow!(elem, env));
            }
            Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(values)))
        }
        EarlyComptimeArrayElements::Repeat { value, count } => {
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

fn eval_resolved_array_literal_flow(
    elems: &ResolvedComptimeArrayElements,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match elems.kind() {
        ResolvedComptimeArrayElementsKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_resolved_value_or_return_flow!(elem, env));
            }
            Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(values)))
        }
        ResolvedComptimeArrayElementsKind::Repeat { value, count } => {
            let value = eval_resolved_value_or_return_flow!(value, env);
            let count_span = count.span();
            let count_value = match eval_resolved_value_or_return_flow!(count, env) {
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
    lhs: &EarlyComptimeExpr,
    index: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_array_index_flow(
    span: Span,
    lhs: &ResolvedComptimeExpr,
    index: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let values = match eval_resolved_value_or_return_flow!(lhs, env) {
        ComptimeValue::Array(values) => values,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime index access requires an array value".to_string(),
            });
        }
    };
    let index_span = index.span();
    let index_value = match eval_resolved_value_or_return_flow!(index, env) {
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

fn eval_array_slice_flow(
    span: Span,
    lhs: &EarlyComptimeExpr,
    range: &EarlyComptimeSliceRange,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let values = match eval_value_or_return_flow!(lhs, env) {
        ComptimeValue::Array(values) => values,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime slicing requires an array value".to_string(),
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
    if range.inclusive {
        end = end.checked_add(1).ok_or_else(|| ComptimeError {
            span,
            message: "comptime slice inclusive end is too large".to_string(),
        })?;
    }
    if start > end || end > len {
        return Err(ComptimeError {
            span,
            message: format!("comptime slice range {start}..{end} is out of bounds"),
        });
    }
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(
        values[start..end].to_vec(),
    )))
}

fn eval_resolved_array_slice_flow(
    span: Span,
    lhs: &ResolvedComptimeExpr,
    range: &ResolvedComptimeSliceRange,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let values = match eval_resolved_value_or_return_flow!(lhs, env) {
        ComptimeValue::Array(values) => values,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime slicing requires an array value".to_string(),
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
        end = end.checked_add(1).ok_or_else(|| ComptimeError {
            span,
            message: "comptime slice inclusive end is too large".to_string(),
        })?;
    }
    if start > end || end > len {
        return Err(ComptimeError {
            span,
            message: format!("comptime slice range {start}..{end} is out of bounds"),
        });
    }
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Array(
        values[start..end].to_vec(),
    )))
}

enum SliceBoundFlow {
    Value(usize),
    Flow(ComptimeEvalFlow),
}

fn eval_slice_bound_flow(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<SliceBoundFlow, ComptimeError> {
    let span = expr.span;
    let value = match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let value = match value {
        ComptimeValue::Int(value) => value,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime slice bound must be an integer".to_string(),
            });
        }
    };
    let value = int_to_array_len(span, value)?;
    usize::try_from(value)
        .map_err(|_| ComptimeError {
            span,
            message: "comptime slice bound is too large".to_string(),
        })
        .map(SliceBoundFlow::Value)
}

fn eval_resolved_slice_bound_flow(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<SliceBoundFlow, ComptimeError> {
    let span = expr.span();
    let value = match eval_resolved_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let value = match value {
        ComptimeValue::Int(value) => value,
        _ => {
            return Err(ComptimeError {
                span,
                message: "comptime slice bound must be an integer".to_string(),
            });
        }
    };
    let value = int_to_array_len(span, value)?;
    usize::try_from(value)
        .map_err(|_| ComptimeError {
            span,
            message: "comptime slice bound is too large".to_string(),
        })
        .map(SliceBoundFlow::Value)
}

fn eval_struct_literal_flow(
    fields: &[nia_comptime_ir::EarlyComptimeFieldInit],
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if let Some(field) = check_unique_field_set(
        fields
            .iter()
            .map(|field| NamedField::new(field.span, field.name.as_str())),
    )
    .into_iter()
    .next()
    {
        return Err(ComptimeError {
            span: field.span,
            message: format!("duplicate comptime struct field `{}`", field.name),
        });
    }
    let mut values = BTreeMap::new();
    for field in fields {
        values.insert(
            field.name.clone(),
            eval_value_or_return_flow!(&field.value, env),
        );
    }
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Struct(values)))
}

fn eval_resolved_struct_literal_flow(
    fields: &[ResolvedComptimeFieldInit],
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if let Some(field) = check_unique_field_set(
        fields
            .iter()
            .map(|field| NamedField::new(field.span(), field.name())),
    )
    .into_iter()
    .next()
    {
        return Err(ComptimeError {
            span: field.span,
            message: format!("duplicate comptime struct field `{}`", field.name),
        });
    }
    let mut values = BTreeMap::new();
    for field in fields {
        values.insert(
            field.name().to_string(),
            eval_resolved_value_or_return_flow!(field.value(), env),
        );
    }
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Struct(values)))
}

fn eval_comptime_int_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<IntConst, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Int(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to an integer".to_string(),
        }),
    }
}

pub fn eval_early_comptime_int_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<IntConst, ComptimeError> {
    eval_comptime_int_expr(expr, env)
}

pub fn eval_resolved_comptime_int_expr(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<IntConst, ComptimeError> {
    eval_resolved_comptime_int_expr_inner(expr, env)
}

fn eval_resolved_comptime_int_expr_inner(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<IntConst, ComptimeError> {
    match eval_resolved_comptime_expr_value(expr, env)? {
        ComptimeValue::Int(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to an integer".to_string(),
        }),
    }
}

fn eval_comptime_bool_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<bool, ComptimeError> {
    match eval_comptime_expr(expr, env)? {
        ComptimeValue::Bool(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to bool".to_string(),
        }),
    }
}

pub fn eval_early_comptime_bool_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<bool, ComptimeError> {
    eval_comptime_bool_expr(expr, env)
}

pub fn eval_resolved_comptime_bool_expr(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<bool, ComptimeError> {
    eval_resolved_comptime_bool_expr_inner(expr, env)
}

fn eval_resolved_comptime_bool_expr_inner(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<bool, ComptimeError> {
    match eval_resolved_comptime_expr_value(expr, env)? {
        ComptimeValue::Bool(value) => Ok(value),
        _ => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to bool".to_string(),
        }),
    }
}

fn eval_comptime_array_len_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<u64, ComptimeError> {
    int_to_array_len(expr.span, eval_comptime_int_expr(expr, env)?)
}

pub fn eval_early_comptime_array_len_expr(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<u64, ComptimeError> {
    eval_comptime_array_len_expr(expr, env)
}

pub fn eval_resolved_comptime_array_len_expr(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<u64, ComptimeError> {
    int_to_array_len(
        expr.span(),
        eval_resolved_comptime_int_expr_inner(expr, env)?,
    )
}

fn eval_comptime_function_call(
    span: Span,
    function_module_id: ModuleId,
    params: &[EarlyComptimeParam],
    body: &EarlyComptimeBlock,
    type_substitutions: Vec<(String, InternedTyId)>,
    args: Vec<ComptimeValue>,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    if let ArityCheck::Mismatch { actual, .. } = check_exact_arity(params.len(), args.len()) {
        return Err(ComptimeError {
            span,
            message: format!(
                "comptime function argument count mismatch: expected {}, got {}",
                params.len(),
                actual
            ),
        });
    }
    env.push_comptime_scope(span)?;
    if let Err(err) = env.bind_function_context(span, function_module_id, None, type_substitutions)
    {
        env.pop_comptime_scope();
        return Err(err);
    }
    for (param, value) in params.iter().zip(args) {
        if let Err(err) = env.bind_function_param(param.span, param, value) {
            env.pop_comptime_scope();
            return Err(err);
        }
    }
    let result = eval_function_block(body, env).and_then(|flow| match flow {
        ComptimeEvalFlow::Value(value)
        | ComptimeEvalFlow::Return(value)
        | ComptimeEvalFlow::Propagate(value) => Ok(value),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: body.span,
            message: "comptime loop control flow escaped its loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: body.span,
            message: "comptime function must return a value".to_string(),
        }),
    });
    env.pop_comptime_scope();
    result
}

pub fn eval_early_comptime_function_call(
    span: Span,
    function_module_id: ModuleId,
    function: &EarlyComptimeFunction,
    type_substitutions: Vec<(String, InternedTyId)>,
    args: Vec<ComptimeValue>,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    eval_comptime_function_call(
        span,
        function_module_id,
        &function.params,
        &function.body,
        type_substitutions,
        args,
        env,
    )
}

pub fn eval_resolved_comptime_function_call(
    span: Span,
    function_id: GlobalDefId,
    function_module_id: ModuleId,
    function: &ResolvedComptimeFunction,
    type_substitutions: Vec<(String, InternedTyId)>,
    args: Vec<ComptimeValue>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    eval_resolved_comptime_function_call_inner(
        ResolvedComptimeCall {
            span,
            function_id,
            function_module_id,
            params: function.params(),
            body: function.body(),
            type_substitutions,
            args,
        },
        env,
    )
}

struct ResolvedComptimeCall<'a> {
    span: Span,
    function_id: GlobalDefId,
    function_module_id: ModuleId,
    params: &'a [ResolvedComptimeParam],
    body: &'a ResolvedComptimeBlock,
    type_substitutions: Vec<(String, InternedTyId)>,
    args: Vec<ComptimeValue>,
}

fn eval_resolved_comptime_function_call_inner(
    call: ResolvedComptimeCall<'_>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let ResolvedComptimeCall {
        span,
        function_id,
        function_module_id,
        params,
        body,
        type_substitutions,
        args,
    } = call;
    if let ArityCheck::Mismatch { actual, .. } = check_exact_arity(params.len(), args.len()) {
        return Err(ComptimeError {
            span,
            message: format!(
                "comptime function argument count mismatch: expected {}, got {}",
                params.len(),
                actual
            ),
        });
    }
    env.push_comptime_scope(span)?;
    if let Err(err) = env.bind_function_context(
        span,
        function_module_id,
        Some(function_id),
        type_substitutions,
    ) {
        env.pop_comptime_scope();
        return Err(err);
    }
    for (param, value) in params.iter().zip(args) {
        if let Err(err) = env.bind_resolved_function_param(param.span(), param, value) {
            env.pop_comptime_scope();
            return Err(err);
        }
    }
    let result = eval_resolved_function_block(body, env).and_then(|flow| match flow {
        ComptimeEvalFlow::Value(value)
        | ComptimeEvalFlow::Return(value)
        | ComptimeEvalFlow::Propagate(value) => Ok(value),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: body.span(),
            message: "comptime loop control flow escaped its loop".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: body.span(),
            message: "comptime function must return a value".to_string(),
        }),
    });
    env.pop_comptime_scope();
    result
}

fn eval_function_block(
    block: &EarlyComptimeBlock,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if block.stmts.is_empty() {
        return eval_function_block_without_scope(block, env);
    }
    env.push_comptime_scope(block.span)?;
    let result = eval_function_block_without_scope(block, env);
    env.pop_comptime_scope();
    result
}

fn eval_resolved_function_block(
    block: &ResolvedComptimeBlock,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    if block.is_empty() {
        return eval_resolved_function_block_without_scope(block, env);
    }
    env.push_comptime_scope(block.span())?;
    let result = eval_resolved_function_block_without_scope(block, env);
    env.pop_comptime_scope();
    result
}

fn eval_resolved_function_block_without_scope(
    block: &ResolvedComptimeBlock,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for stmt in block.stmts() {
        match eval_resolved_function_stmt(stmt, env)? {
            ComptimeEvalFlow::Return(value) => return Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => return Ok(ComptimeEvalFlow::Propagate(value)),
            ComptimeEvalFlow::Break => return Ok(ComptimeEvalFlow::Break),
            ComptimeEvalFlow::Continue => return Ok(ComptimeEvalFlow::Continue),
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void => {}
        }
    }
    block.tail().map_or(Ok(ComptimeEvalFlow::Void), |tail| {
        eval_resolved_function_tail_expr(tail, env)
    })
}

fn eval_resolved_function_tail_expr(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    eval_resolved_comptime_expr_flow(expr, env)
}

fn eval_function_block_without_scope(
    block: &EarlyComptimeBlock,
    env: &mut impl EarlyComptimeEnv,
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
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    eval_comptime_expr_flow(expr, env)
}

fn eval_function_stmt(
    stmt: &EarlyComptimeStmt,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match &stmt.kind {
        EarlyComptimeStmtKind::Binding(binding) => {
            match eval_comptime_expr_flow(&binding.value, env)? {
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
            }
        }
        EarlyComptimeStmtKind::Expr(expr) => match eval_comptime_expr_flow(expr, env)? {
            ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void => Ok(ComptimeEvalFlow::Void),
            ComptimeEvalFlow::Return(value) => Ok(ComptimeEvalFlow::Return(value)),
            ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Propagate(value)),
            ComptimeEvalFlow::Break => Ok(ComptimeEvalFlow::Break),
            ComptimeEvalFlow::Continue => Ok(ComptimeEvalFlow::Continue),
        },
        EarlyComptimeStmtKind::Return(value) => {
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
        EarlyComptimeStmtKind::Break => Ok(ComptimeEvalFlow::Break),
        EarlyComptimeStmtKind::Continue => Ok(ComptimeEvalFlow::Continue),
        EarlyComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => eval_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        EarlyComptimeStmtKind::ForIn(for_in) => eval_for_in_stmt(stmt.span, for_in, env),
        EarlyComptimeStmtKind::While { cond, body } => eval_while_stmt(stmt.span, cond, body, env),
        EarlyComptimeStmtKind::Loop { body } => eval_loop_stmt(stmt.span, body, env),
    }
}

fn eval_resolved_function_stmt(
    stmt: &ResolvedComptimeStmt,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match stmt.kind() {
        ResolvedComptimeStmtKind::Binding(binding) => {
            match eval_resolved_comptime_expr_flow(binding.value(), env)? {
                ComptimeEvalFlow::Value(value) => {
                    env.bind_resolved_function_local(stmt.span(), binding, value)?;
                    Ok(ComptimeEvalFlow::Void)
                }
                ComptimeEvalFlow::Return(value) => Ok(ComptimeEvalFlow::Return(value)),
                ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Propagate(value)),
                ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
                    span: stmt.span(),
                    message: "comptime binding value cannot contain loop control flow".to_string(),
                }),
                ComptimeEvalFlow::Void => Err(ComptimeError {
                    span: stmt.span(),
                    message: "comptime function binding requires a value".to_string(),
                }),
            }
        }
        ResolvedComptimeStmtKind::Expr(expr) => {
            match eval_resolved_comptime_expr_flow(expr, env)? {
                ComptimeEvalFlow::Value(_) | ComptimeEvalFlow::Void => Ok(ComptimeEvalFlow::Void),
                ComptimeEvalFlow::Return(value) => Ok(ComptimeEvalFlow::Return(value)),
                ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Propagate(value)),
                ComptimeEvalFlow::Break => Ok(ComptimeEvalFlow::Break),
                ComptimeEvalFlow::Continue => Ok(ComptimeEvalFlow::Continue),
            }
        }
        ResolvedComptimeStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ComptimeError {
                    span: stmt.span(),
                    message: "comptime function must return a value".to_string(),
                });
            };
            match eval_resolved_comptime_expr_flow(value, env)? {
                ComptimeEvalFlow::Value(value)
                | ComptimeEvalFlow::Return(value)
                | ComptimeEvalFlow::Propagate(value) => Ok(ComptimeEvalFlow::Return(value)),
                ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
                    span: stmt.span(),
                    message: "comptime return value cannot contain loop control flow".to_string(),
                }),
                ComptimeEvalFlow::Void => Err(ComptimeError {
                    span: stmt.span(),
                    message: "comptime function must return a value".to_string(),
                }),
            }
        }
        ResolvedComptimeStmtKind::Break => Ok(ComptimeEvalFlow::Break),
        ResolvedComptimeStmtKind::Continue => Ok(ComptimeEvalFlow::Continue),
        ResolvedComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => eval_resolved_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        ResolvedComptimeStmtKind::ForIn(for_in) => {
            eval_resolved_for_in_stmt(stmt.span(), for_in, env)
        }
        ResolvedComptimeStmtKind::While { cond, body } => {
            eval_resolved_while_stmt(stmt.span(), cond, body, env)
        }
        ResolvedComptimeStmtKind::Loop { body } => eval_resolved_loop_stmt(stmt.span(), body, env),
    }
}

fn eval_assign_expr_flow(
    span: Span,
    assign: &EarlyComptimeAssign,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_assign_expr_flow(
    span: Span,
    assign: &ResolvedComptimeAssign,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let value = match eval_resolved_assignment_value_flow(span, assign, env)? {
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
    let value = resolved_assign_target_writeback_value(span, assign.lhs(), value, env)?;
    env.assign_resolved_local(span, assign.lhs(), value)?;
    Ok(ComptimeEvalFlow::Void)
}

fn eval_assignment_value_flow(
    span: Span,
    assign: &EarlyComptimeAssign,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let rhs = eval_value_or_return_flow!(&assign.rhs, env);
    if matches!(assign.op, ComptimeAssignOp::Assign) {
        return Ok(ComptimeEvalFlow::Value(rhs));
    }
    let lhs = eval_assign_target_value(span, &assign.lhs, env)?;
    let op = assign_op_binary(assign.op).ok_or_else(|| ComptimeError {
        span,
        message: "unsupported comptime assignment operator".to_string(),
    })?;
    eval_numeric_binary_value(lhs, op, rhs)
        .map(ComptimeEvalFlow::Value)
        .map_err(|message| ComptimeError { span, message })
}

fn eval_resolved_assignment_value_flow(
    span: Span,
    assign: &ResolvedComptimeAssign,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let rhs = eval_resolved_value_or_return_flow!(assign.rhs(), env);
    if matches!(assign.op(), ComptimeAssignOp::Assign) {
        return Ok(ComptimeEvalFlow::Value(rhs));
    }
    let lhs = eval_resolved_assign_target_value(span, assign.lhs(), env)?;
    let op = assign_op_binary(assign.op()).ok_or_else(|| ComptimeError {
        span,
        message: "unsupported comptime assignment operator".to_string(),
    })?;
    eval_numeric_binary_value(lhs, op, rhs)
        .map(ComptimeEvalFlow::Value)
        .map_err(|message| ComptimeError { span, message })
}

fn eval_assign_target_root_value(
    span: Span,
    target: &EarlyComptimeAssignTarget,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target {
        EarlyComptimeAssignTarget::Local {
            span: target_span,
            name,
            local_id,
            ..
        } => {
            let Some(local_id) = local_id else {
                return Err(ComptimeError {
                    span,
                    message: format!("failed to resolve comptime assignment target `{name}`"),
                });
            };
            env.resolve_name(
                *target_span,
                &EarlyComptimeName::resolved(
                    name.clone(),
                    ComptimeNameResolution::Local(*local_id),
                ),
            )
        }
    }
}

fn eval_resolved_assign_target_root_value(
    target: &ResolvedComptimeAssignTarget,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target.kind() {
        ResolvedComptimeAssignTargetKind::Local { span, local_id, .. } => {
            env.resolve_resolved_name(*span, ComptimeNameResolution::Local(*local_id))
        }
    }
}

fn eval_assign_target_value(
    span: Span,
    target: &EarlyComptimeAssignTarget,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let value = eval_assign_target_root_value(span, target, env)?;
    match target {
        EarlyComptimeAssignTarget::Local { path, .. } => {
            eval_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_resolved_assign_target_value(
    span: Span,
    target: &ResolvedComptimeAssignTarget,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let value = eval_resolved_assign_target_root_value(target, env)?;
    match target.kind() {
        ResolvedComptimeAssignTargetKind::Local { path, .. } => {
            eval_resolved_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_assign_path_value(
    span: Span,
    mut value: ComptimeValue,
    path: &[EarlyComptimeAssignPathElem],
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    for elem in path {
        value = match elem {
            EarlyComptimeAssignPathElem::Field { span, name } => match value {
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
            EarlyComptimeAssignPathElem::Index {
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

fn eval_resolved_assign_path_value(
    span: Span,
    mut value: ComptimeValue,
    path: &[ResolvedComptimeAssignPathElem],
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    for elem in path {
        value = match elem.kind() {
            ResolvedComptimeAssignPathElemKind::Field { span, name } => match value {
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
            ResolvedComptimeAssignPathElemKind::Index {
                span: elem_span,
                index,
            } => match value {
                ComptimeValue::Array(values) => {
                    let index = eval_resolved_assign_path_index(*elem_span, index, env)?;
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
    target: &EarlyComptimeAssignTarget,
    value: ComptimeValue,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target {
        EarlyComptimeAssignTarget::Local { path, .. } => {
            if path.is_empty() {
                return Ok(value);
            }
            let root = eval_assign_target_root_value(span, target, env)?;
            write_assign_path_value(span, root, path, value, env)
        }
    }
}

fn resolved_assign_target_writeback_value(
    span: Span,
    target: &ResolvedComptimeAssignTarget,
    value: ComptimeValue,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    match target.kind() {
        ResolvedComptimeAssignTargetKind::Local { path, .. } => {
            if path.is_empty() {
                return Ok(value);
            }
            let root = eval_resolved_assign_target_root_value(target, env)?;
            write_resolved_assign_path_value(span, root, path, value, env)
        }
    }
}

fn write_assign_path_value(
    span: Span,
    root: ComptimeValue,
    path: &[EarlyComptimeAssignPathElem],
    value: ComptimeValue,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        EarlyComptimeAssignPathElem::Field {
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
        EarlyComptimeAssignPathElem::Index {
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

fn write_resolved_assign_path_value(
    span: Span,
    root: ComptimeValue,
    path: &[ResolvedComptimeAssignPathElem],
    value: ComptimeValue,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeValue, ComptimeError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head.kind() {
        ResolvedComptimeAssignPathElemKind::Field {
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
            let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
            fields.insert(name.clone(), updated);
            Ok(ComptimeValue::Struct(fields))
        }
        ResolvedComptimeAssignPathElemKind::Index {
            span: index_span,
            index,
        } => {
            let ComptimeValue::Array(mut values) = root else {
                return Err(ComptimeError {
                    span: *index_span,
                    message: "comptime index assignment requires an array value".to_string(),
                });
            };
            let index = eval_resolved_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ComptimeError {
                    span,
                    message: format!("comptime array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ComptimeValue::Array(values))
        }
    }
}

fn eval_assign_path_index(
    span: Span,
    index: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_assign_path_index(
    span: Span,
    index: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<usize, ComptimeError> {
    let index_span = index.span();
    let value = match eval_resolved_comptime_expr_flow(index, env)? {
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

fn assign_op_binary(op: ComptimeAssignOp) -> Option<ComptimeBinaryOp> {
    Some(match op {
        ComptimeAssignOp::Assign => return None,
        ComptimeAssignOp::Add => ComptimeBinaryOp::Add,
        ComptimeAssignOp::Sub => ComptimeBinaryOp::Sub,
        ComptimeAssignOp::Shl => ComptimeBinaryOp::Shl,
        ComptimeAssignOp::Shr => ComptimeBinaryOp::Shr,
        ComptimeAssignOp::Mul => ComptimeBinaryOp::Mul,
        ComptimeAssignOp::Div => ComptimeBinaryOp::Div,
        ComptimeAssignOp::Rem => ComptimeBinaryOp::Rem,
        ComptimeAssignOp::BitAnd => ComptimeBinaryOp::BitAnd,
        ComptimeAssignOp::BitXor => ComptimeBinaryOp::BitXor,
        ComptimeAssignOp::BitOr => ComptimeBinaryOp::BitOr,
    })
}

fn eval_range_expr_flow(
    range: &EarlyComptimeRange,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_range_expr_flow(
    range: &ResolvedComptimeRange,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let start = match eval_resolved_optional_range_bound(range.start(), env)? {
        ComptimeRangeBoundFlow::Value(value) => value,
        ComptimeRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    let end = match eval_resolved_optional_range_bound(range.end(), env)? {
        ComptimeRangeBoundFlow::Value(value) => value,
        ComptimeRangeBoundFlow::Flow(flow) => return Ok(flow),
    };
    Ok(ComptimeEvalFlow::Value(ComptimeValue::Range(
        ComptimeRangeValue {
            start,
            end,
            inclusive: range.is_inclusive(),
        },
    )))
}

enum ComptimeRangeBoundFlow {
    Value(Option<IntConst>),
    Flow(ComptimeEvalFlow),
}

fn eval_optional_range_bound(
    expr: Option<&EarlyComptimeExpr>,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    let Some(expr) = expr else {
        return Ok(ComptimeRangeBoundFlow::Value(None));
    };
    eval_range_bound(expr, env)
}

fn eval_resolved_optional_range_bound(
    expr: Option<&ResolvedComptimeExpr>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    let Some(expr) = expr else {
        return Ok(ComptimeRangeBoundFlow::Value(None));
    };
    eval_resolved_range_bound(expr, env)
}

fn eval_range_bound(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Int(value)) => {
            Ok(ComptimeRangeBoundFlow::Value(Some(value)))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound must be an integer".to_string(),
        }),
        ComptimeEvalFlow::Return(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Return(value),
        )),
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound requires a value".to_string(),
        }),
    }
}

fn eval_resolved_range_bound(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeRangeBoundFlow, ComptimeError> {
    match eval_resolved_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Int(value)) => {
            Ok(ComptimeRangeBoundFlow::Value(Some(value)))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound must be an integer".to_string(),
        }),
        ComptimeEvalFlow::Return(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Return(value),
        )),
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeRangeBoundFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime range bound requires a value".to_string(),
        }),
    }
}

fn eval_for_in_stmt(
    span: Span,
    for_in: &EarlyComptimeForIn,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match eval_comptime_expr_flow(&for_in.iter, env)? {
        ComptimeEvalFlow::Value(_) => {}
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => return Ok(flow),
        ComptimeEvalFlow::Void => {
            return Err(ComptimeError {
                span: for_in.iter.span(),
                message: "comptime for-in iterator requires a value".to_string(),
            });
        }
    }
    let _ = span;
    let _ = &for_in.binding;
    let _ = &for_in.body;
    let _ = env;
    Err(ComptimeError {
        span: for_in.iter.span(),
        message: "comptime for-in Iterator execution is not implemented yet".to_string(),
    })
}

fn eval_resolved_for_in_stmt(
    span: Span,
    for_in: &ResolvedComptimeForIn,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    match eval_resolved_comptime_expr_flow(for_in.iter(), env)? {
        ComptimeEvalFlow::Value(_) => {}
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => return Ok(flow),
        ComptimeEvalFlow::Void => {
            return Err(ComptimeError {
                span: for_in.iter().span(),
                message: "comptime for-in iterator requires a value".to_string(),
            });
        }
    }
    let _ = span;
    let _ = for_in.binding();
    let _ = for_in.body();
    let _ = env;
    Err(ComptimeError {
        span: for_in.iter().span(),
        message: "comptime for-in Iterator execution is not implemented yet".to_string(),
    })
}

fn eval_while_stmt(
    span: Span,
    cond: &EarlyComptimeExpr,
    body: &EarlyComptimeBlock,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_while_stmt(
    span: Span,
    cond: &ResolvedComptimeExpr,
    body: &ResolvedComptimeBlock,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for _ in 0..COMPTIME_LOOP_LIMIT {
        let cond_value = match eval_resolved_condition_flow(
            cond,
            env,
            "comptime while condition must evaluate to bool",
        )? {
            ComptimeConditionFlow::Value(value) => value,
            ComptimeConditionFlow::Flow(flow) => return Ok(flow),
        };
        if !cond_value {
            return Ok(ComptimeEvalFlow::Void);
        }
        match eval_resolved_function_block(body, env)? {
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
    body: &EarlyComptimeBlock,
    env: &mut impl EarlyComptimeEnv,
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

fn eval_resolved_loop_stmt(
    span: Span,
    body: &ResolvedComptimeBlock,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    for _ in 0..COMPTIME_LOOP_LIMIT {
        match eval_resolved_function_block(body, env)? {
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
    cond: &EarlyComptimeExpr,
    then_branch: &EarlyComptimeBlock,
    else_branch: Option<&EarlyComptimeBlock>,
    env: &mut impl EarlyComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let cond_value = match eval_condition_flow(cond, env, "if condition must evaluate to bool")? {
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

fn eval_resolved_if_stmt(
    cond: &ResolvedComptimeExpr,
    then_branch: &ResolvedComptimeBlock,
    else_branch: Option<&ResolvedComptimeBlock>,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    let cond_value =
        match eval_resolved_condition_flow(cond, env, "if condition must evaluate to bool")? {
            ComptimeConditionFlow::Value(value) => value,
            ComptimeConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        eval_resolved_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ComptimeEvalFlow::Void), |else_branch| {
            eval_resolved_function_block(else_branch, env)
        })
    }
}

fn eval_condition_flow(
    cond: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
    type_error: &'static str,
) -> Result<ComptimeConditionFlow, ComptimeError> {
    match eval_comptime_expr_flow(cond, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Bool(value)) => {
            Ok(ComptimeConditionFlow::Value(value))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: cond.span(),
            message: type_error.to_string(),
        }),
        ComptimeEvalFlow::Return(value) => {
            Ok(ComptimeConditionFlow::Flow(ComptimeEvalFlow::Return(value)))
        }
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeConditionFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: cond.span(),
            message: "comptime condition cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: cond.span(),
            message: "comptime condition requires a value".to_string(),
        }),
    }
}

fn eval_resolved_condition_flow(
    cond: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
    type_error: &'static str,
) -> Result<ComptimeConditionFlow, ComptimeError> {
    match eval_resolved_comptime_expr_flow(cond, env)? {
        ComptimeEvalFlow::Value(ComptimeValue::Bool(value)) => {
            Ok(ComptimeConditionFlow::Value(value))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: cond.span(),
            message: type_error.to_string(),
        }),
        ComptimeEvalFlow::Return(value) => {
            Ok(ComptimeConditionFlow::Flow(ComptimeEvalFlow::Return(value)))
        }
        ComptimeEvalFlow::Propagate(value) => Ok(ComptimeConditionFlow::Flow(
            ComptimeEvalFlow::Propagate(value),
        )),
        ComptimeEvalFlow::Break | ComptimeEvalFlow::Continue => Err(ComptimeError {
            span: cond.span(),
            message: "comptime condition cannot contain loop control flow".to_string(),
        }),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: cond.span(),
            message: "comptime condition requires a value".to_string(),
        }),
    }
}

fn comptime_bit_not(value: IntConst) -> IntConst {
    if value.is_signed() {
        IntConst::from_i128(!value.as_i128().unwrap_or(value.bits() as i128))
    } else {
        IntConst::unsigned(!value.bits())
    }
}

fn int_to_array_len(span: Span, value: IntConst) -> Result<u64, ComptimeError> {
    let Some(value) = value.as_i128() else {
        return Err(ComptimeError {
            span,
            message: "array length is too large".to_string(),
        });
    };
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

fn int_to_i128(value: IntConst, context: &str) -> Result<i128, String> {
    value
        .as_i128()
        .ok_or_else(|| format!("integer value is too large for {context}"))
}

fn eval_binary_int(lhs: IntConst, op: ComptimeBinaryOp, rhs: IntConst) -> Result<IntConst, String> {
    if !lhs.is_signed() && !rhs.is_signed() {
        return eval_binary_uint(lhs.bits(), op, rhs.bits()).map(IntConst::unsigned);
    }
    let lhs = int_to_i128(lhs, "comptime operation")?;
    let rhs = int_to_i128(rhs, "comptime operation")?;
    Ok(match op {
        ComptimeBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in comptime multiplication".to_string())?,
        ComptimeBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in comptime expression".to_string());
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| "integer overflow in comptime division".to_string())?
        }
        ComptimeBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in comptime expression".to_string());
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| "integer overflow in comptime remainder".to_string())?
        }
        ComptimeBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in comptime addition".to_string())?,
        ComptimeBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in comptime subtraction".to_string())?,
        ComptimeBinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        ComptimeBinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        ComptimeBinaryOp::BitAnd => lhs & rhs,
        ComptimeBinaryOp::BitXor => lhs ^ rhs,
        ComptimeBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in comptime expression: {op:?}"
            ));
        }
    }
    .into())
}

fn eval_binary_uint(lhs: u128, op: ComptimeBinaryOp, rhs: u128) -> Result<u128, String> {
    Ok(match op {
        ComptimeBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in comptime multiplication".to_string())?,
        ComptimeBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in comptime expression".to_string());
            }
            lhs / rhs
        }
        ComptimeBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in comptime expression".to_string());
            }
            lhs % rhs
        }
        ComptimeBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in comptime addition".to_string())?,
        ComptimeBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in comptime subtraction".to_string())?,
        ComptimeBinaryOp::Shl => checked_shift_u128(lhs, rhs, true)?,
        ComptimeBinaryOp::Shr => checked_shift_u128(lhs, rhs, false)?,
        ComptimeBinaryOp::BitAnd => lhs & rhs,
        ComptimeBinaryOp::BitXor => lhs ^ rhs,
        ComptimeBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in comptime expression: {op:?}"
            ));
        }
    })
}

fn eval_numeric_binary_value(
    lhs: ComptimeValue,
    op: ComptimeBinaryOp,
    rhs: ComptimeValue,
) -> Result<ComptimeValue, String> {
    match (lhs, rhs) {
        (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => {
            eval_binary_int(lhs, op, rhs).map(ComptimeValue::Int)
        }
        (ComptimeValue::Float(lhs), ComptimeValue::Float(rhs)) => eval_binary_float(lhs, op, rhs),
        _ => Err("comptime numeric operation requires matching operand types".to_string()),
    }
}

fn eval_numeric_operand_flow(
    expr: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
) -> Result<Result<ComptimeValue, ComptimeEvalFlow>, ComptimeError> {
    match eval_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value @ (ComptimeValue::Int(_) | ComptimeValue::Float(_))) => {
            Ok(Ok(value))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to a numeric value".to_string(),
        }),
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => Ok(Err(flow)),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression requires a value".to_string(),
        }),
    }
}

fn eval_binary_flow(
    span: Span,
    lhs: &EarlyComptimeExpr,
    op: ComptimeBinaryOp,
    rhs: &EarlyComptimeExpr,
    env: &mut impl EarlyComptimeEnv,
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
    let value = match op {
        ComptimeBinaryOp::And => {
            let lhs = bool_operand!(lhs);
            if !lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(false)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        ComptimeBinaryOp::Or => {
            let lhs = bool_operand!(lhs);
            if lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(true)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        ComptimeBinaryOp::Eq | ComptimeBinaryOp::Ne => {
            let lhs = eval_value_or_return_flow!(lhs, env);
            let rhs = eval_value_or_return_flow!(rhs, env);
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ComptimeError {
                span,
                message: "comptime equality requires matching operand types".to_string(),
            })?;
            ComptimeValue::Bool(if op == ComptimeBinaryOp::Eq {
                equal
            } else {
                !equal
            })
        }
        ComptimeBinaryOp::Lt
        | ComptimeBinaryOp::Le
        | ComptimeBinaryOp::Gt
        | ComptimeBinaryOp::Ge => {
            let lhs = match eval_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            match (lhs, rhs) {
                (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => {
                    ComptimeValue::Bool(eval_binary_int_compare(lhs, op, rhs))
                }
                (ComptimeValue::Float(lhs), ComptimeValue::Float(rhs)) => {
                    eval_binary_float(lhs, op, rhs)
                        .map_err(|message| ComptimeError { span, message })?
                }
                _ => {
                    return Err(ComptimeError {
                        span,
                        message: "comptime comparison requires matching operand types".to_string(),
                    });
                }
            }
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
                .map_err(|message| ComptimeError { span, message })?
        }
    };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_resolved_numeric_operand_flow(
    expr: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<Result<ComptimeValue, ComptimeEvalFlow>, ComptimeError> {
    match eval_resolved_comptime_expr_flow(expr, env)? {
        ComptimeEvalFlow::Value(value @ (ComptimeValue::Int(_) | ComptimeValue::Float(_))) => {
            Ok(Ok(value))
        }
        ComptimeEvalFlow::Value(_) => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression must evaluate to a numeric value".to_string(),
        }),
        flow @ (ComptimeEvalFlow::Return(_)
        | ComptimeEvalFlow::Propagate(_)
        | ComptimeEvalFlow::Break
        | ComptimeEvalFlow::Continue) => Ok(Err(flow)),
        ComptimeEvalFlow::Void => Err(ComptimeError {
            span: expr.span(),
            message: "comptime expression requires a value".to_string(),
        }),
    }
}

fn eval_resolved_binary_flow(
    span: Span,
    lhs: &ResolvedComptimeExpr,
    op: ComptimeBinaryOp,
    rhs: &ResolvedComptimeExpr,
    env: &mut impl ResolvedComptimeEnv,
) -> Result<ComptimeEvalFlow, ComptimeError> {
    macro_rules! bool_operand {
        ($expr:expr) => {
            match eval_resolved_value_or_return_flow!($expr, env) {
                ComptimeValue::Bool(value) => value,
                _ => {
                    return Err(ComptimeError {
                        span: $expr.span(),
                        message: "comptime expression must evaluate to bool".to_string(),
                    });
                }
            }
        };
    }
    let value = match op {
        ComptimeBinaryOp::And => {
            let lhs = bool_operand!(lhs);
            if !lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(false)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        ComptimeBinaryOp::Or => {
            let lhs = bool_operand!(lhs);
            if lhs {
                return Ok(ComptimeEvalFlow::Value(ComptimeValue::Bool(true)));
            }
            ComptimeValue::Bool(bool_operand!(rhs))
        }
        ComptimeBinaryOp::Eq | ComptimeBinaryOp::Ne => {
            let lhs = eval_resolved_value_or_return_flow!(lhs, env);
            let rhs = eval_resolved_value_or_return_flow!(rhs, env);
            let equal = values_equal(&lhs, &rhs).ok_or_else(|| ComptimeError {
                span,
                message: "comptime equality requires matching operand types".to_string(),
            })?;
            ComptimeValue::Bool(if op == ComptimeBinaryOp::Eq {
                equal
            } else {
                !equal
            })
        }
        ComptimeBinaryOp::Lt
        | ComptimeBinaryOp::Le
        | ComptimeBinaryOp::Gt
        | ComptimeBinaryOp::Ge => {
            let lhs = match eval_resolved_numeric_operand_flow(lhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            let rhs = match eval_resolved_numeric_operand_flow(rhs, env)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            match (lhs, rhs) {
                (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => {
                    ComptimeValue::Bool(eval_binary_int_compare(lhs, op, rhs))
                }
                (ComptimeValue::Float(lhs), ComptimeValue::Float(rhs)) => {
                    eval_binary_float(lhs, op, rhs)
                        .map_err(|message| ComptimeError { span, message })?
                }
                _ => {
                    return Err(ComptimeError {
                        span,
                        message: "comptime comparison requires matching operand types".to_string(),
                    });
                }
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
            eval_numeric_binary_value(lhs, op, rhs)
                .map_err(|message| ComptimeError { span, message })?
        }
    };
    Ok(ComptimeEvalFlow::Value(value))
}

fn eval_binary_int_compare(lhs: IntConst, op: ComptimeBinaryOp, rhs: IntConst) -> bool {
    if !lhs.is_signed() && !rhs.is_signed() {
        return match op {
            ComptimeBinaryOp::Lt => lhs.bits() < rhs.bits(),
            ComptimeBinaryOp::Le => lhs.bits() <= rhs.bits(),
            ComptimeBinaryOp::Gt => lhs.bits() > rhs.bits(),
            ComptimeBinaryOp::Ge => lhs.bits() >= rhs.bits(),
            _ => unreachable!("non-comparison binary operator routed to integer comparison"),
        };
    }
    let lhs = lhs.as_i128().unwrap_or(i128::MAX);
    let rhs = rhs.as_i128().unwrap_or(i128::MAX);
    match op {
        ComptimeBinaryOp::Lt => lhs < rhs,
        ComptimeBinaryOp::Le => lhs <= rhs,
        ComptimeBinaryOp::Gt => lhs > rhs,
        ComptimeBinaryOp::Ge => lhs >= rhs,
        _ => unreachable!("non-comparison binary operator routed to integer comparison"),
    }
}

fn eval_binary_float(lhs: f64, op: ComptimeBinaryOp, rhs: f64) -> Result<ComptimeValue, String> {
    Ok(match op {
        ComptimeBinaryOp::Add => ComptimeValue::Float(lhs + rhs),
        ComptimeBinaryOp::Sub => ComptimeValue::Float(lhs - rhs),
        ComptimeBinaryOp::Mul => ComptimeValue::Float(lhs * rhs),
        ComptimeBinaryOp::Div => ComptimeValue::Float(lhs / rhs),
        ComptimeBinaryOp::Rem => ComptimeValue::Float(lhs % rhs),
        ComptimeBinaryOp::Lt => ComptimeValue::Bool(lhs < rhs),
        ComptimeBinaryOp::Le => ComptimeValue::Bool(lhs <= rhs),
        ComptimeBinaryOp::Gt => ComptimeValue::Bool(lhs > rhs),
        ComptimeBinaryOp::Ge => ComptimeValue::Bool(lhs >= rhs),
        _ => {
            return Err(format!(
                "unsupported binary operator for float comptime expression: {op:?}"
            ));
        }
    })
}

fn values_equal(lhs: &ComptimeValue, rhs: &ComptimeValue) -> Option<bool> {
    match (lhs, rhs) {
        (ComptimeValue::Int(lhs), ComptimeValue::Int(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Float(lhs), ComptimeValue::Float(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Bool(lhs), ComptimeValue::Bool(rhs)) => Some(lhs == rhs),
        (ComptimeValue::String(lhs), ComptimeValue::String(rhs)) => Some(lhs == rhs),
        (ComptimeValue::Pointer(lhs), ComptimeValue::Pointer(rhs)) => values_equal(lhs, rhs),
        (ComptimeValue::Pointer(lhs), rhs) => values_equal(lhs, rhs),
        (lhs, ComptimeValue::Pointer(rhs)) => values_equal(lhs, rhs),
        (ComptimeValue::String(lhs), ComptimeValue::Array(rhs)) => {
            Some(char_array_to_string(rhs)? == *lhs)
        }
        (ComptimeValue::Array(lhs), ComptimeValue::String(rhs)) => {
            Some(char_array_to_string(lhs)? == *rhs)
        }
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
