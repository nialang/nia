use crate::{
    ConstError, ConstRangeValue, ConstValue, EarlyConstEnv, ResolvedConstEnv,
    literals::{
        bytes_to_array, char_array_to_string, checked_shift, checked_shift_u128,
        const_error_message, decode_byte_char_literal, decode_char_literal,
        eval_byte_string_literal, eval_float_literal, eval_int_literal, eval_string_literal,
        string_to_char_array,
    },
};

use nia_const_ir::{
    ConstAssignOp, ConstBinaryOp, ConstNameResolution, ConstUnaryOp, EarlyConstArrayElements,
    EarlyConstAssign, EarlyConstAssignPathElem, EarlyConstAssignTarget, EarlyConstBlock,
    EarlyConstExpr, EarlyConstExprKind, EarlyConstForIn, EarlyConstFunction, EarlyConstName,
    EarlyConstParam, EarlyConstPattern, EarlyConstRange, EarlyConstSliceRange, EarlyConstStmt,
    EarlyConstStmtKind, EarlyConstSwitch, EarlyConstSwitchArm, EarlyConstSwitchArmBody,
    ResolvedConstArrayElements, ResolvedConstArrayElementsKind, ResolvedConstAssign,
    ResolvedConstAssignPathElem, ResolvedConstAssignPathElemKind, ResolvedConstAssignTarget,
    ResolvedConstAssignTargetKind, ResolvedConstBlock, ResolvedConstExpr, ResolvedConstExprKind,
    ResolvedConstFieldInit, ResolvedConstForIn, ResolvedConstFunction, ResolvedConstParam,
    ResolvedConstPatternKind, ResolvedConstRange, ResolvedConstSliceRange, ResolvedConstStmt,
    ResolvedConstStmtKind, ResolvedConstSwitch, ResolvedConstSwitchArm, ResolvedConstSwitchArmBody,
    ResolvedConstSwitchArmBodyKind,
};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId};
use nia_sema::{ArityCheck, NamedField, check_exact_arity, check_unique_field_set};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::IntConst;
use std::collections::BTreeMap;

enum ConstEvalFlow {
    Value(ConstValue),
    Return(ConstValue),
    Propagate(ConstValue),
    Break,
    Continue,
    Void,
}

const CONST_LOOP_LIMIT: usize = 100_000;

macro_rules! eval_value_or_return_flow {
    ($expr:expr, $env:expr) => {
        match eval_const_expr_flow($expr, $env)? {
            ConstEvalFlow::Value(value) => value,
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => {
                return Ok(flow);
            }
            ConstEvalFlow::Void => {
                return Err(ConstError {
                    span: $expr.span,
                    message: "const expression requires a value".to_string(),
                });
            }
        }
    };
}

struct ConstSwitchMatch<'a> {
    arm: &'a EarlyConstSwitchArm,
    bindings: Vec<ConstSwitchBinding>,
}

struct ConstSwitchBinding {
    span: Span,
    name: SymbolId,
    local_id: Option<nia_ids::LocalId>,
    value: ConstValue,
}
fn eval_const_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    match eval_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => Ok(value),
        ConstEvalFlow::Return(_) => Err(ConstError {
            span: expr.span(),
            message: "const expression cannot return from a const function".to_string(),
        }),
        ConstEvalFlow::Propagate(_) => Err(ConstError {
            span: expr.span(),
            message: "const `.?` propagation requires a const function".to_string(),
        }),
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: expr.span(),
            message: "const loop control flow requires an enclosing loop".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const expression requires a value".to_string(),
        }),
    }
}

pub fn eval_early_const_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    eval_const_expr(expr, env)
}

pub fn eval_resolved_const_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    eval_resolved_const_expr_value(expr, env)
}

fn eval_resolved_const_expr_value(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    match eval_resolved_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => Ok(value),
        ConstEvalFlow::Return(_) => Err(ConstError {
            span: expr.span(),
            message: "const expression cannot return from a const function".to_string(),
        }),
        ConstEvalFlow::Propagate(_) => Err(ConstError {
            span: expr.span(),
            message: "const `.?` propagation requires a const function".to_string(),
        }),
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: expr.span(),
            message: "const loop control flow requires an enclosing loop".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const expression requires a value".to_string(),
        }),
    }
}

macro_rules! eval_resolved_value_or_return_flow {
    ($expr:expr, $env:expr) => {
        match eval_resolved_const_expr_flow($expr, $env)? {
            ConstEvalFlow::Value(value) => value,
            flow @ (ConstEvalFlow::Return(_)
            | ConstEvalFlow::Propagate(_)
            | ConstEvalFlow::Break
            | ConstEvalFlow::Continue) => {
                return Ok(flow);
            }
            ConstEvalFlow::Void => {
                return Err(ConstError {
                    span: $expr.span(),
                    message: "const expression requires a value".to_string(),
                });
            }
        }
    };
}

fn eval_resolved_const_expr_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let span = expr.span();
    let value = match expr.kind() {
        ResolvedConstExprKind::Bool(value) => ConstValue::Bool(*value),
        ResolvedConstExprKind::Null => ConstValue::Optional(None),
        ResolvedConstExprKind::String(literal) => eval_string_literal(literal)
            .map(|value| ConstValue::Array(string_to_char_array(&value)))
            .ok_or_else(|| ConstError {
                span,
                message: "unsupported string literal in const expression".to_string(),
            })?,
        ResolvedConstExprKind::ByteString(literal) => eval_byte_string_literal(literal)
            .map(|value| ConstValue::Array(bytes_to_array(&value)))
            .ok_or_else(|| ConstError {
                span,
                message: "unsupported byte string literal in const expression".to_string(),
            })?,
        ResolvedConstExprKind::Embed { path } => {
            let path = eval_string_literal(path).ok_or_else(|| ConstError {
                span,
                message: "invalid `embed` path literal".to_string(),
            })?;
            env.resolve_embed(span, &path)?
        }
        ResolvedConstExprKind::Integer(text) => eval_int_literal(text)
            .map(|value| ConstValue::Int(IntConst::from_i128(value)))
            .map_err(|message| ConstError { span, message })?,
        ResolvedConstExprKind::Float(text) => eval_float_literal(text)
            .map(ConstValue::Float)
            .map_err(|message| ConstError { span, message })?,
        ResolvedConstExprKind::Char(text) => decode_char_literal(text)
            .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ConstError {
                span,
                message: format!("invalid char literal `{text}` in const expression"),
            })?,
        ResolvedConstExprKind::ByteChar(text) => decode_byte_char_literal(text)
            .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ConstError {
                span,
                message: format!("invalid byte char literal `{text}` in const expression"),
            })?,
        ResolvedConstExprKind::Name(resolution) => {
            env.resolve_resolved_name(span, resolution.clone())?
        }
        ResolvedConstExprKind::Field { lhs, name } => {
            match eval_resolved_value_or_return_flow!(lhs, env) {
                ConstValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("unknown const field `{}`", env.symbol_name(*name)),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span,
                        message: "const field access requires a struct value".to_string(),
                    });
                }
            }
        }
        ResolvedConstExprKind::BuiltinMethod { method, lhs } => {
            eval_builtin_method_value(span, *method, eval_resolved_value_or_return_flow!(lhs, env))?
        }
        ResolvedConstExprKind::Index { lhs, index } => {
            return eval_resolved_array_index_flow(span, lhs, index, env);
        }
        ResolvedConstExprKind::Slice { lhs, range } => {
            return eval_resolved_array_slice_flow(span, lhs, range, env);
        }
        ResolvedConstExprKind::ArrayLiteral { elems, .. } => {
            return eval_resolved_array_literal_flow(elems, env);
        }
        ResolvedConstExprKind::StructLiteral { fields, .. } => {
            return eval_resolved_struct_literal_flow(fields, env);
        }
        ResolvedConstExprKind::CompileError { message } => {
            let value = eval_resolved_value_or_return_flow!(message, env);
            let Some(message) = const_error_message(&value) else {
                return Err(ConstError {
                    span,
                    message: "builtin `error` requires a const string message".to_string(),
                });
            };
            return Err(ConstError { span, message });
        }
        ResolvedConstExprKind::LayoutBuiltin { builtin, type_arg } => {
            env.resolve_resolved_layout_builtin(span, *builtin, type_arg)?
        }
        ResolvedConstExprKind::FieldOffsetBuiltin { type_arg, field } => {
            env.resolve_resolved_field_offset_builtin(span, type_arg, field)?
        }
        ResolvedConstExprKind::BuiltinConstValue(builtin) => {
            env.resolve_builtin_const(span, *builtin)?
        }
        ResolvedConstExprKind::BuiltinValue(builtin) => {
            env.resolve_builtin_value(span, *builtin)?
        }
        ResolvedConstExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            if let ResolvedConstExprKind::BuiltinValue(builtin) = callee.kind() {
                if !args.is_empty() {
                    return Err(ConstError {
                        span,
                        message: format!(
                            "unsupported builtin call in const expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_builtin_value(span, *builtin)?
            } else if let ResolvedConstExprKind::LayoutBuiltin { builtin, type_arg } = callee.kind()
            {
                if !args.is_empty() {
                    return Err(ConstError {
                        span,
                        message: format!(
                            "unsupported builtin call in const expression: @{}",
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
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::Neg,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ConstValue::Int(value) => value
                .as_i128()
                .and_then(i128::checked_neg)
                .map(|value| ConstValue::Int(IntConst::from_i128(value)))
                .ok_or_else(|| ConstError {
                    span,
                    message: "integer overflow in const negation".to_string(),
                })?,
            ConstValue::Float(value) => ConstValue::Float(-value),
            _ => {
                return Err(ConstError {
                    span,
                    message: "const negation requires a numeric value".to_string(),
                });
            }
        },
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::Not,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ConstValue::Bool(value) => ConstValue::Bool(!value),
            _ => {
                return Err(ConstError {
                    span,
                    message: "const `not` requires a bool".to_string(),
                });
            }
        },
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::BitNot,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ConstValue::Int(value) => ConstValue::Int(const_bit_not(value)),
            _ => {
                return Err(ConstError {
                    span,
                    message: "const bitwise not requires an integer".to_string(),
                });
            }
        },
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::Deref,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ConstValue::Pointer(value) => *value,
            _ => {
                return Err(ConstError {
                    span,
                    message: "const dereference requires a pointer value".to_string(),
                });
            }
        },
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref,
            expr: inner,
        } => ConstValue::Pointer(Box::new(eval_resolved_value_or_return_flow!(inner, env))),
        ResolvedConstExprKind::OptionalSome { expr: inner } => ConstValue::Optional(Some(
            Box::new(eval_resolved_value_or_return_flow!(inner, env)),
        )),
        ResolvedConstExprKind::ErrorOk { expr: inner } => ConstValue::ErrorUnion(Ok(Box::new(
            eval_resolved_value_or_return_flow!(inner, env),
        ))),
        ResolvedConstExprKind::ErrorErr { expr: inner } => ConstValue::ErrorUnion(Err(Box::new(
            eval_resolved_value_or_return_flow!(inner, env),
        ))),
        ResolvedConstExprKind::Try { expr: inner } => {
            return eval_resolved_try_expr_flow(span, inner, env);
        }
        ResolvedConstExprKind::Binary { lhs, op, rhs } => {
            return eval_resolved_binary_flow(span, lhs, *op, rhs, env);
        }
        ResolvedConstExprKind::Assign(assign) => {
            return eval_resolved_assign_expr_flow(span, assign, env);
        }
        ResolvedConstExprKind::Range(range) => {
            return eval_resolved_range_expr_flow(range, env);
        }
        ResolvedConstExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            return eval_resolved_const_if_expr_flow(
                span,
                cond,
                then_branch,
                else_branch.as_deref(),
                env,
            );
        }
        ResolvedConstExprKind::Switch(switch) => {
            return eval_resolved_const_switch_expr_flow(switch, env);
        }
        ResolvedConstExprKind::Cast { expr: inner, ty } => {
            let value = eval_resolved_value_or_return_flow!(inner, env);
            env.cast_value(span, value, *ty)?
        }
        ResolvedConstExprKind::Block(block) => {
            return eval_resolved_function_block(block, env);
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_const_expr_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let value = match &expr.kind {
        EarlyConstExprKind::Bool(value) => ConstValue::Bool(*value),
        EarlyConstExprKind::Null => ConstValue::Optional(None),
        EarlyConstExprKind::String(literal) => eval_string_literal(literal)
            .map(|value| ConstValue::Array(string_to_char_array(&value)))
            .ok_or_else(|| ConstError {
                span: expr.span,
                message: "unsupported string literal in const expression".to_string(),
            })?,
        EarlyConstExprKind::ByteString(literal) => eval_byte_string_literal(literal)
            .map(|value| ConstValue::Array(bytes_to_array(&value)))
            .ok_or_else(|| ConstError {
                span: expr.span,
                message: "unsupported byte string literal in const expression".to_string(),
            })?,
        EarlyConstExprKind::Embed { path } => {
            let path = eval_string_literal(path).ok_or_else(|| ConstError {
                span: expr.span,
                message: "invalid `embed` path literal".to_string(),
            })?;
            env.resolve_embed(expr.span, &path)?
        }
        EarlyConstExprKind::Integer(text) => eval_int_literal(text)
            .map(|value| ConstValue::Int(IntConst::from_i128(value)))
            .map_err(|message| ConstError {
                span: expr.span,
                message,
            })?,
        EarlyConstExprKind::Float(text) => eval_float_literal(text)
            .map(ConstValue::Float)
            .map_err(|message| ConstError {
                span: expr.span,
                message,
            })?,
        EarlyConstExprKind::Char(text) => decode_char_literal(text)
            .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ConstError {
                span: expr.span,
                message: format!("invalid char literal `{text}` in const expression"),
            })?,
        EarlyConstExprKind::ByteChar(text) => decode_byte_char_literal(text)
            .map(|value| ConstValue::Int(IntConst::unsigned(value as u128)))
            .ok_or_else(|| ConstError {
                span: expr.span,
                message: format!("invalid byte char literal `{text}` in const expression"),
            })?,
        EarlyConstExprKind::Ident(name) | EarlyConstExprKind::Qualified(name) => {
            env.resolve_name(expr.span, name)?
        }
        EarlyConstExprKind::Field { lhs, name } => match eval_value_or_return_flow!(lhs, env) {
            ConstValue::Struct(fields) => fields.get(name).cloned().ok_or_else(|| ConstError {
                span: expr.span,
                message: format!("unknown const field `{}`", env.symbol_name(*name)),
            })?,
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const field access requires a struct value".to_string(),
                });
            }
        },
        EarlyConstExprKind::BuiltinMethod { method, lhs } => {
            eval_builtin_method_value(expr.span, *method, eval_value_or_return_flow!(lhs, env))?
        }
        EarlyConstExprKind::Index { lhs, index } => {
            return eval_array_index_flow(expr.span, lhs, index, env);
        }
        EarlyConstExprKind::Slice { lhs, range } => {
            return eval_array_slice_flow(expr.span, lhs, range, env);
        }
        EarlyConstExprKind::ArrayLiteral { elems, .. } => {
            return eval_array_literal_flow(elems, env);
        }
        EarlyConstExprKind::StructLiteral { fields, .. } => {
            return eval_struct_literal_flow(fields, env);
        }
        EarlyConstExprKind::CompileError { message } => {
            let value = eval_value_or_return_flow!(message, env);
            let Some(message) = const_error_message(&value) else {
                return Err(ConstError {
                    span: expr.span,
                    message: "builtin `error` requires a const string message".to_string(),
                });
            };
            return Err(ConstError {
                span: expr.span,
                message,
            });
        }
        EarlyConstExprKind::LayoutBuiltin { builtin, type_arg } => {
            env.resolve_layout_builtin(expr.span, *builtin, type_arg)?
        }
        EarlyConstExprKind::FieldOffsetBuiltin { type_arg, field } => {
            env.resolve_field_offset_builtin(expr.span, type_arg, field)?
        }
        EarlyConstExprKind::BuiltinConstValue(builtin) => {
            env.resolve_builtin_const(expr.span, *builtin)?
        }
        EarlyConstExprKind::BuiltinValue(builtin) => {
            env.resolve_builtin_value(expr.span, *builtin)?
        }
        EarlyConstExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            if let EarlyConstExprKind::BuiltinValue(builtin) = &callee.kind {
                if !args.is_empty() {
                    return Err(ConstError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin call in const expression: @{}",
                            builtin.name()
                        ),
                    });
                }
                env.resolve_builtin_value(expr.span, *builtin)?
            } else if let EarlyConstExprKind::LayoutBuiltin { builtin, type_arg } = &callee.kind {
                if !args.is_empty() {
                    return Err(ConstError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin call in const expression: @{}",
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
        EarlyConstExprKind::Unary {
            op: ConstUnaryOp::Neg,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ConstValue::Int(value) => value
                .as_i128()
                .and_then(i128::checked_neg)
                .map(|value| ConstValue::Int(IntConst::from_i128(value)))
                .ok_or_else(|| ConstError {
                    span: expr.span,
                    message: "integer overflow in const negation".to_string(),
                })?,
            ConstValue::Float(value) => ConstValue::Float(-value),
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const negation requires a numeric value".to_string(),
                });
            }
        },
        EarlyConstExprKind::Unary {
            op: ConstUnaryOp::Not,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ConstValue::Bool(value) => ConstValue::Bool(!value),
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const `not` requires a bool".to_string(),
                });
            }
        },
        EarlyConstExprKind::Unary {
            op: ConstUnaryOp::BitNot,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ConstValue::Int(value) => ConstValue::Int(const_bit_not(value)),
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const bitwise not requires an integer".to_string(),
                });
            }
        },
        EarlyConstExprKind::Unary {
            op: ConstUnaryOp::Deref,
            expr: inner,
        } => match eval_value_or_return_flow!(inner, env) {
            ConstValue::Pointer(value) => *value,
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const dereference requires a pointer value".to_string(),
                });
            }
        },
        EarlyConstExprKind::Unary {
            op: ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref,
            expr: inner,
        } => ConstValue::Pointer(Box::new(eval_value_or_return_flow!(inner, env))),
        EarlyConstExprKind::OptionalSome { expr: inner } => {
            ConstValue::Optional(Some(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyConstExprKind::ErrorOk { expr: inner } => {
            ConstValue::ErrorUnion(Ok(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyConstExprKind::ErrorErr { expr: inner } => {
            ConstValue::ErrorUnion(Err(Box::new(eval_value_or_return_flow!(inner, env))))
        }
        EarlyConstExprKind::Try { expr: inner } => {
            return eval_try_expr_flow(expr.span, inner, env);
        }
        EarlyConstExprKind::Binary { lhs, op, rhs } => {
            return eval_binary_flow(expr.span, lhs, *op, rhs, env);
        }
        EarlyConstExprKind::Assign(assign) => {
            return eval_assign_expr_flow(expr.span, assign, env);
        }
        EarlyConstExprKind::Range(range) => {
            return eval_range_expr_flow(range, env);
        }
        EarlyConstExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            return eval_const_if_expr_flow(
                expr.span,
                cond,
                then_branch,
                else_branch.as_deref(),
                env,
            );
        }
        EarlyConstExprKind::Switch(switch) => {
            return eval_const_switch_expr_flow(switch, env);
        }
        EarlyConstExprKind::Cast {
            expr: inner,
            ty: Some(ty),
        } => {
            let value = eval_value_or_return_flow!(inner, env);
            env.cast_value(expr.span, value, *ty)?
        }
        EarlyConstExprKind::Cast {
            expr: inner,
            ty: None,
        } => eval_value_or_return_flow!(inner, env),
        EarlyConstExprKind::Block(block) => {
            return eval_function_block(block, env);
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_const_if_expr_flow(
    span: Span,
    cond: &EarlyConstExpr,
    then_branch: &EarlyConstBlock,
    else_branch: Option<&EarlyConstExpr>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let cond_value = match eval_condition_flow(cond, env, "const expression must evaluate to bool")?
    {
        ConstConditionFlow::Value(value) => value,
        ConstConditionFlow::Flow(flow) => return Ok(flow),
    };
    if cond_value {
        return eval_function_block(then_branch, env);
    }
    if let Some(else_branch) = else_branch {
        eval_const_expr_flow(else_branch, env)
    } else {
        Err(ConstError {
            span,
            message: "if expression requires an else branch".to_string(),
        })
    }
}

fn eval_builtin_method_value(
    span: Span,
    method: BuiltinTraitMethod,
    value: ConstValue,
) -> Result<ConstValue, ConstError> {
    match method {
        BuiltinTraitMethod::Len => eval_builtin_len_value(span, value),
        BuiltinTraitMethod::Start => eval_builtin_range_bound_value(span, value, true),
        BuiltinTraitMethod::End => eval_builtin_range_bound_value(span, value, false),
        _ => Err(ConstError {
            span,
            message: format!(
                "unsupported builtin trait method in const expression: {}",
                method.name()
            ),
        }),
    }
}

fn eval_builtin_len_value(span: Span, value: ConstValue) -> Result<ConstValue, ConstError> {
    match value {
        ConstValue::Array(values) => Ok(ConstValue::Int(IntConst::unsigned(
            u128::try_from(values.len()).map_err(|_| ConstError {
                span,
                message: "const array length is too large".to_string(),
            })?,
        ))),
        _ => Err(ConstError {
            span,
            message: "const len requires an array value".to_string(),
        }),
    }
}

fn eval_builtin_range_bound_value(
    span: Span,
    value: ConstValue,
    want_start: bool,
) -> Result<ConstValue, ConstError> {
    let ConstValue::Range(range) = value else {
        return Err(ConstError {
            span,
            message: "const range bound method requires a range value".to_string(),
        });
    };
    let bound = if want_start { range.start } else { range.end };
    let Some(bound) = bound else {
        let name = if want_start { "start" } else { "end" };
        return Err(ConstError {
            span,
            message: format!("const range does not have a {name} bound"),
        });
    };
    Ok(ConstValue::Int(bound))
}

fn eval_const_switch_expr_flow(
    switch: &EarlyConstSwitch,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let target = eval_value_or_return_flow!(&switch.target, env);
    let Some(matched) = matching_switch_arm(&target, switch, env)? else {
        return Err(ConstError {
            span: switch.span,
            message: "const switch expression did not match any arm".to_string(),
        });
    };
    eval_const_switch_match_body(matched, env)
}

fn eval_resolved_const_if_expr_flow(
    span: Span,
    cond: &ResolvedConstExpr,
    then_branch: &ResolvedConstBlock,
    else_branch: Option<&ResolvedConstExpr>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let cond_value =
        match eval_resolved_condition_flow(cond, env, "const expression must evaluate to bool")? {
            ConstConditionFlow::Value(value) => value,
            ConstConditionFlow::Flow(flow) => return Ok(flow),
        };
    if cond_value {
        return eval_resolved_function_block(then_branch, env);
    }
    if let Some(else_branch) = else_branch {
        eval_resolved_const_expr_flow(else_branch, env)
    } else {
        Err(ConstError {
            span,
            message: "if expression requires an else branch".to_string(),
        })
    }
}

struct ResolvedConstSwitchMatch<'a> {
    arm: &'a ResolvedConstSwitchArm,
    bindings: Vec<ConstSwitchBinding>,
}

fn eval_resolved_const_switch_expr_flow(
    switch: &ResolvedConstSwitch,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let target = eval_resolved_value_or_return_flow!(switch.target(), env);
    let Some(matched) = matching_resolved_switch_arm(&target, switch, env)? else {
        return Err(ConstError {
            span: switch.span(),
            message: "const switch expression did not match any arm".to_string(),
        });
    };
    eval_resolved_const_switch_match_body(matched, env)
}

fn eval_try_expr_flow(
    span: Span,
    inner: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match eval_const_expr_flow(inner, env)? {
        ConstEvalFlow::Value(ConstValue::Optional(Some(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::Optional(None)) => {
            Ok(ConstEvalFlow::Propagate(ConstValue::Optional(None)))
        }
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Ok(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Err(value))) => {
            Ok(ConstEvalFlow::Propagate(ConstValue::ErrorUnion(Err(value))))
        }
        ConstEvalFlow::Value(_) => Err(ConstError {
            span,
            message: "const `.?` requires optional or error union operand".to_string(),
        }),
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => Ok(flow),
        ConstEvalFlow::Void => Err(ConstError {
            span,
            message: "const `.?` requires a value".to_string(),
        }),
    }
}

fn eval_resolved_try_expr_flow(
    span: Span,
    inner: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match eval_resolved_const_expr_flow(inner, env)? {
        ConstEvalFlow::Value(ConstValue::Optional(Some(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::Optional(None)) => {
            Ok(ConstEvalFlow::Propagate(ConstValue::Optional(None)))
        }
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Ok(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Err(value))) => {
            Ok(ConstEvalFlow::Propagate(ConstValue::ErrorUnion(Err(value))))
        }
        ConstEvalFlow::Value(_) => Err(ConstError {
            span,
            message: "const `.?` requires optional or error union operand".to_string(),
        }),
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => Ok(flow),
        ConstEvalFlow::Void => Err(ConstError {
            span,
            message: "const `.?` requires a value".to_string(),
        }),
    }
}

fn matching_switch_arm<'a>(
    target: &ConstValue,
    switch: &'a EarlyConstSwitch,
    env: &mut impl EarlyConstEnv,
) -> Result<Option<ConstSwitchMatch<'a>>, ConstError> {
    let mut default = None;
    for arm in &switch.arms {
        for pattern in &arm.patterns {
            if matches!(pattern, EarlyConstPattern::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if early_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ConstSwitchMatch { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ConstSwitchMatch {
        arm,
        bindings: Vec::new(),
    }))
}

fn matching_resolved_switch_arm<'a>(
    target: &ConstValue,
    switch: &'a ResolvedConstSwitch,
    env: &mut impl ResolvedConstEnv,
) -> Result<Option<ResolvedConstSwitchMatch<'a>>, ConstError> {
    let mut default = None;
    for arm in switch.arms() {
        for pattern in arm.patterns() {
            if matches!(pattern.kind(), ResolvedConstPatternKind::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if resolved_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ResolvedConstSwitchMatch { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ResolvedConstSwitchMatch {
        arm,
        bindings: Vec::new(),
    }))
}

fn early_pattern_matches(
    target: &ConstValue,
    pattern: &EarlyConstPattern,
    env: &mut impl EarlyConstEnv,
    bindings: &mut Vec<ConstSwitchBinding>,
) -> Result<bool, ConstError> {
    match pattern {
        EarlyConstPattern::Wildcard { .. } => Ok(true),
        EarlyConstPattern::Bind {
            name,
            local_id,
            span,
        } => {
            bindings.push(ConstSwitchBinding {
                span: *span,
                name: *name,
                local_id: *local_id,
                value: target.clone(),
            });
            Ok(true)
        }
        EarlyConstPattern::Pointer { pattern, span }
        | EarlyConstPattern::MutPointer { pattern, span } => match target {
            ConstValue::Pointer(value) => early_pattern_matches(value, pattern, env, bindings),
            _ => Err(ConstError {
                span: *span,
                message: "const pointer pattern requires a pointer target".to_string(),
            }),
        },
        EarlyConstPattern::OptionalSome { pattern, span } => match target {
            ConstValue::Optional(Some(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::Optional(None) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const optional switch pattern requires an optional target".to_string(),
            }),
        },
        EarlyConstPattern::OptionalNull { span } => match target {
            ConstValue::Optional(None) => Ok(true),
            ConstValue::Optional(Some(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const null switch pattern requires an optional target".to_string(),
            }),
        },
        EarlyConstPattern::ErrorOk { pattern, span } => match target {
            ConstValue::ErrorUnion(Ok(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error-ok switch pattern requires an error union target".to_string(),
            }),
        },
        EarlyConstPattern::ErrorErr { pattern, span } => match target {
            ConstValue::ErrorUnion(Err(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error switch pattern requires an error union target".to_string(),
            }),
        },
        EarlyConstPattern::Expr(pattern) => {
            let pattern = eval_const_expr(pattern, env)?;
            Ok(values_equal(target, &pattern).unwrap_or(false))
        }
        EarlyConstPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn resolved_pattern_matches(
    target: &ConstValue,
    pattern: &nia_const_ir::ResolvedConstPattern,
    env: &mut impl ResolvedConstEnv,
    bindings: &mut Vec<ConstSwitchBinding>,
) -> Result<bool, ConstError> {
    match pattern.kind() {
        ResolvedConstPatternKind::Wildcard { .. } => Ok(true),
        ResolvedConstPatternKind::Bind {
            name,
            local_id,
            span,
        } => {
            bindings.push(ConstSwitchBinding {
                span: *span,
                name: *name,
                local_id: Some(*local_id),
                value: target.clone(),
            });
            Ok(true)
        }
        ResolvedConstPatternKind::Pointer { pattern, span }
        | ResolvedConstPatternKind::MutPointer { pattern, span } => match target {
            ConstValue::Pointer(value) => resolved_pattern_matches(value, pattern, env, bindings),
            _ => Err(ConstError {
                span: *span,
                message: "const pointer pattern requires a pointer target".to_string(),
            }),
        },
        ResolvedConstPatternKind::OptionalSome { pattern, span } => match target {
            ConstValue::Optional(Some(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::Optional(None) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const optional switch pattern requires an optional target".to_string(),
            }),
        },
        ResolvedConstPatternKind::OptionalNull { span } => match target {
            ConstValue::Optional(None) => Ok(true),
            ConstValue::Optional(Some(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const null switch pattern requires an optional target".to_string(),
            }),
        },
        ResolvedConstPatternKind::ErrorOk { pattern, span } => match target {
            ConstValue::ErrorUnion(Ok(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error-ok switch pattern requires an error union target".to_string(),
            }),
        },
        ResolvedConstPatternKind::ErrorErr { pattern, span } => match target {
            ConstValue::ErrorUnion(Err(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error switch pattern requires an error union target".to_string(),
            }),
        },
        ResolvedConstPatternKind::Expr(pattern) => {
            let pattern = eval_resolved_const_expr_value(pattern, env)?;
            Ok(values_equal(target, &pattern).unwrap_or(false))
        }
        ResolvedConstPatternKind::Range {
            start,
            end,
            inclusive,
            span,
        } => resolved_switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn switch_range_matches(
    target: &ConstValue,
    start: &EarlyConstExpr,
    end: &EarlyConstExpr,
    inclusive: bool,
    span: Span,
    env: &mut impl EarlyConstEnv,
) -> Result<bool, ConstError> {
    let ConstValue::Int(target) = target else {
        return Err(ConstError {
            span,
            message: "const switch range requires an integer target".to_string(),
        });
    };
    let start = eval_const_int_expr(start, env)?;
    let end = eval_const_int_expr(end, env)?;
    Ok(if inclusive {
        eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ConstBinaryOp::Le, end)
    } else {
        eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ConstBinaryOp::Lt, end)
    })
}

fn resolved_switch_range_matches(
    target: &ConstValue,
    start: &ResolvedConstExpr,
    end: &ResolvedConstExpr,
    inclusive: bool,
    span: Span,
    env: &mut impl ResolvedConstEnv,
) -> Result<bool, ConstError> {
    let ConstValue::Int(target) = target else {
        return Err(ConstError {
            span,
            message: "const switch range requires an integer target".to_string(),
        });
    };
    let start = eval_resolved_const_int_expr_inner(start, env)?;
    let end = eval_resolved_const_int_expr_inner(end, env)?;
    Ok(if inclusive {
        eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ConstBinaryOp::Le, end)
    } else {
        eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && eval_binary_int_compare(*target, ConstBinaryOp::Lt, end)
    })
}

fn eval_const_switch_arm_body(
    body: &EarlyConstSwitchArmBody,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match body {
        EarlyConstSwitchArmBody::Expr(expr) => eval_function_tail_expr(expr, env),
        EarlyConstSwitchArmBody::Stmt(stmt) => eval_function_stmt(stmt, env),
        EarlyConstSwitchArmBody::Block(block) => eval_function_block(block, env),
    }
}

fn eval_const_switch_match_body(
    matched: ConstSwitchMatch<'_>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if matched.bindings.is_empty() {
        return eval_const_switch_arm_body(&matched.arm.body, env);
    }
    env.push_const_scope(matched.arm.span)?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| bind_pattern_value(binding, env));
    let result = bind_result.and_then(|()| eval_const_switch_arm_body(&matched.arm.body, env));
    env.pop_const_scope();
    result
}

fn eval_resolved_const_switch_arm_body(
    body: &ResolvedConstSwitchArmBody,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match body.kind() {
        ResolvedConstSwitchArmBodyKind::Expr(expr) => eval_resolved_function_tail_expr(expr, env),
        ResolvedConstSwitchArmBodyKind::Stmt(stmt) => eval_resolved_function_stmt(stmt, env),
        ResolvedConstSwitchArmBodyKind::Block(block) => eval_resolved_function_block(block, env),
    }
}

fn eval_resolved_const_switch_match_body(
    matched: ResolvedConstSwitchMatch<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if matched.bindings.is_empty() {
        return eval_resolved_const_switch_arm_body(matched.arm.body(), env);
    }
    env.push_const_scope(matched.arm.span())?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| bind_resolved_pattern_value(binding, env));
    let result =
        bind_result.and_then(|()| eval_resolved_const_switch_arm_body(matched.arm.body(), env));
    env.pop_const_scope();
    result
}

fn bind_pattern_value(
    binding: &ConstSwitchBinding,
    env: &mut impl EarlyConstEnv,
) -> Result<(), ConstError> {
    env.bind_pattern_local(
        binding.span,
        &binding.name,
        binding.local_id,
        binding.value.clone(),
    )
}

fn bind_resolved_pattern_value(
    binding: &ConstSwitchBinding,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let local_id = binding
        .local_id
        .expect("resolved const switch pattern must have a local id");
    env.bind_resolved_pattern_local(binding.span, &binding.name, local_id, binding.value.clone())
}

fn eval_array_literal_flow(
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
            let count = int_to_array_len(count_span, count_value)?;
            let count = usize::try_from(count).map_err(|_| ConstError {
                span: count_span,
                message: "const array repeat count is too large".to_string(),
            })?;
            Ok(ConstEvalFlow::Value(ConstValue::Array(vec![value; count])))
        }
    }
}

fn eval_resolved_array_literal_flow(
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
            let count = int_to_array_len(count_span, count_value)?;
            let count = usize::try_from(count).map_err(|_| ConstError {
                span: count_span,
                message: "const array repeat count is too large".to_string(),
            })?;
            Ok(ConstEvalFlow::Value(ConstValue::Array(vec![value; count])))
        }
    }
}

fn eval_array_index_flow(
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
    let index = int_to_array_len(index_span, index_value)?;
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

fn eval_resolved_array_index_flow(
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
    let index = int_to_array_len(index_span, index_value)?;
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

fn eval_array_slice_flow(
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

fn eval_resolved_array_slice_flow(
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

enum SliceBoundFlow {
    Value(usize),
    Flow(ConstEvalFlow),
}

fn eval_slice_bound_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<SliceBoundFlow, ConstError> {
    let span = expr.span;
    let value = match eval_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let value = match value {
        ConstValue::Int(value) => value,
        _ => {
            return Err(ConstError {
                span,
                message: "const slice bound must be an integer".to_string(),
            });
        }
    };
    let value = int_to_array_len(span, value)?;
    usize::try_from(value)
        .map_err(|_| ConstError {
            span,
            message: "const slice bound is too large".to_string(),
        })
        .map(SliceBoundFlow::Value)
}

fn eval_resolved_slice_bound_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<SliceBoundFlow, ConstError> {
    let span = expr.span();
    let value = match eval_resolved_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value) => value,
        flow => return Ok(SliceBoundFlow::Flow(flow)),
    };
    let value = match value {
        ConstValue::Int(value) => value,
        _ => {
            return Err(ConstError {
                span,
                message: "const slice bound must be an integer".to_string(),
            });
        }
    };
    let value = int_to_array_len(span, value)?;
    usize::try_from(value)
        .map_err(|_| ConstError {
            span,
            message: "const slice bound is too large".to_string(),
        })
        .map(SliceBoundFlow::Value)
}

fn eval_struct_literal_flow(
    fields: &[nia_const_ir::EarlyConstFieldInit],
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if let Some(field) = check_unique_field_set(
        fields
            .iter()
            .map(|field| NamedField::new(field.span, field.name)),
    )
    .into_iter()
    .next()
    {
        return Err(ConstError {
            span: field.span,
            message: format!(
                "duplicate const struct field `{}`",
                env.symbol_name(field.name)
            ),
        });
    }
    let mut values = BTreeMap::new();
    for field in fields {
        values.insert(field.name, eval_value_or_return_flow!(&field.value, env));
    }
    Ok(ConstEvalFlow::Value(ConstValue::Struct(values)))
}

fn eval_resolved_struct_literal_flow(
    fields: &[ResolvedConstFieldInit],
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if let Some(field) = check_unique_field_set(
        fields
            .iter()
            .map(|field| NamedField::new(field.span(), field.name())),
    )
    .into_iter()
    .next()
    {
        return Err(ConstError {
            span: field.span,
            message: format!(
                "duplicate const struct field `{}`",
                env.symbol_name(field.name)
            ),
        });
    }
    let mut values = BTreeMap::new();
    for field in fields {
        values.insert(
            *field.name_symbol(),
            eval_resolved_value_or_return_flow!(field.value(), env),
        );
    }
    Ok(ConstEvalFlow::Value(ConstValue::Struct(values)))
}

fn eval_const_int_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<IntConst, ConstError> {
    match eval_const_expr(expr, env)? {
        ConstValue::Int(value) => Ok(value),
        _ => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to an integer".to_string(),
        }),
    }
}

pub fn eval_early_const_int_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<IntConst, ConstError> {
    eval_const_int_expr(expr, env)
}

pub fn eval_resolved_const_int_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<IntConst, ConstError> {
    eval_resolved_const_int_expr_inner(expr, env)
}

fn eval_resolved_const_int_expr_inner(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<IntConst, ConstError> {
    match eval_resolved_const_expr_value(expr, env)? {
        ConstValue::Int(value) => Ok(value),
        _ => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to an integer".to_string(),
        }),
    }
}

fn eval_const_bool_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<bool, ConstError> {
    match eval_const_expr(expr, env)? {
        ConstValue::Bool(value) => Ok(value),
        _ => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to bool".to_string(),
        }),
    }
}

pub fn eval_early_const_bool_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<bool, ConstError> {
    eval_const_bool_expr(expr, env)
}

pub fn eval_resolved_const_bool_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<bool, ConstError> {
    eval_resolved_const_bool_expr_inner(expr, env)
}

fn eval_resolved_const_bool_expr_inner(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<bool, ConstError> {
    match eval_resolved_const_expr_value(expr, env)? {
        ConstValue::Bool(value) => Ok(value),
        _ => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to bool".to_string(),
        }),
    }
}

fn eval_const_array_len_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<u64, ConstError> {
    int_to_array_len(expr.span, eval_const_int_expr(expr, env)?)
}

pub fn eval_early_const_array_len_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<u64, ConstError> {
    eval_const_array_len_expr(expr, env)
}

pub fn eval_resolved_const_array_len_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<u64, ConstError> {
    int_to_array_len(expr.span(), eval_resolved_const_int_expr_inner(expr, env)?)
}

struct EarlyConstCall<'a> {
    span: Span,
    function_module_id: ModuleId,
    params: &'a [EarlyConstParam],
    body: &'a EarlyConstBlock,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    args: Vec<ConstValue>,
}

fn eval_const_function_call(
    call: EarlyConstCall<'_>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let EarlyConstCall {
        span,
        function_module_id,
        params,
        body,
        type_substitutions,
        const_substitutions,
        args,
    } = call;
    if let ArityCheck::Mismatch { actual, .. } = check_exact_arity(params.len(), args.len()) {
        return Err(ConstError {
            span,
            message: format!(
                "const function argument count mismatch: expected {}, got {}",
                params.len(),
                actual
            ),
        });
    }
    env.push_const_scope(span)?;
    if let Err(err) = env.bind_function_context(
        span,
        function_module_id,
        None,
        type_substitutions,
        const_substitutions,
    ) {
        env.pop_const_scope();
        return Err(err);
    }
    for (param, value) in params.iter().zip(args) {
        if let Err(err) = env.bind_function_param(param.span, param, value) {
            env.pop_const_scope();
            return Err(err);
        }
    }
    let result = eval_function_block(body, env).and_then(|flow| match flow {
        ConstEvalFlow::Value(value)
        | ConstEvalFlow::Return(value)
        | ConstEvalFlow::Propagate(value) => Ok(value),
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: body.span,
            message: "const loop control flow escaped its loop".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: body.span,
            message: "const function must return a value".to_string(),
        }),
    });
    env.pop_const_scope();
    result
}

pub fn eval_early_const_function_call(
    span: Span,
    function_module_id: ModuleId,
    function: &EarlyConstFunction,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    args: Vec<ConstValue>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    eval_const_function_call(
        EarlyConstCall {
            span,
            function_module_id,
            params: &function.params,
            body: &function.body,
            type_substitutions,
            const_substitutions: Vec::new(),
            args,
        },
        env,
    )
}

pub struct ResolvedConstCallInput<'a> {
    pub span: Span,
    pub function_id: GlobalDefId,
    pub function_module_id: ModuleId,
    pub function: &'a ResolvedConstFunction,
    pub type_substitutions: Vec<(SymbolId, InternedTyId)>,
    pub const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    pub args: Vec<ConstValue>,
}

pub fn eval_resolved_const_function_call(
    input: ResolvedConstCallInput<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let ResolvedConstCallInput {
        span,
        function_id,
        function_module_id,
        function,
        type_substitutions,
        const_substitutions,
        args,
    } = input;
    eval_resolved_const_function_call_inner(
        ResolvedConstCall {
            span,
            function_id,
            function_module_id,
            params: function.params(),
            body: function.body(),
            type_substitutions,
            const_substitutions,
            args,
        },
        env,
    )
}

struct ResolvedConstCall<'a> {
    span: Span,
    function_id: GlobalDefId,
    function_module_id: ModuleId,
    params: &'a [ResolvedConstParam],
    body: &'a ResolvedConstBlock,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    args: Vec<ConstValue>,
}

fn eval_resolved_const_function_call_inner(
    call: ResolvedConstCall<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let ResolvedConstCall {
        span,
        function_id,
        function_module_id,
        params,
        body,
        type_substitutions,
        const_substitutions,
        args,
    } = call;
    if let ArityCheck::Mismatch { actual, .. } = check_exact_arity(params.len(), args.len()) {
        return Err(ConstError {
            span,
            message: format!(
                "const function argument count mismatch: expected {}, got {}",
                params.len(),
                actual
            ),
        });
    }
    env.push_const_scope(span)?;
    if let Err(err) = env.bind_function_context(
        span,
        function_module_id,
        Some(function_id),
        type_substitutions,
        const_substitutions,
    ) {
        env.pop_const_scope();
        return Err(err);
    }
    for (param, value) in params.iter().zip(args) {
        if let Err(err) = env.bind_resolved_function_param(param.span(), param, value) {
            env.pop_const_scope();
            return Err(err);
        }
    }
    let result = eval_resolved_function_block(body, env).and_then(|flow| match flow {
        ConstEvalFlow::Value(value)
        | ConstEvalFlow::Return(value)
        | ConstEvalFlow::Propagate(value) => Ok(value),
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: body.span(),
            message: "const loop control flow escaped its loop".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: body.span(),
            message: "const function must return a value".to_string(),
        }),
    });
    env.pop_const_scope();
    result
}

fn eval_function_block(
    block: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if block.stmts.is_empty() {
        return eval_function_block_without_scope(block, env);
    }
    env.push_const_scope(block.span)?;
    let result = eval_function_block_without_scope(block, env);
    env.pop_const_scope();
    result
}

fn eval_resolved_function_block(
    block: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if block.is_empty() {
        return eval_resolved_function_block_without_scope(block, env);
    }
    env.push_const_scope(block.span())?;
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
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Break),
            ConstEvalFlow::Continue => return Ok(ConstEvalFlow::Continue),
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => {}
        }
    }
    block.tail().map_or(Ok(ConstEvalFlow::Void), |tail| {
        eval_resolved_function_tail_expr(tail, env)
    })
}

fn eval_resolved_function_tail_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    eval_resolved_const_expr_flow(expr, env)
}

fn eval_function_block_without_scope(
    block: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for stmt in &block.stmts {
        match eval_function_stmt(stmt, env)? {
            ConstEvalFlow::Return(value) => return Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => return Ok(ConstEvalFlow::Propagate(value)),
            ConstEvalFlow::Break => return Ok(ConstEvalFlow::Break),
            ConstEvalFlow::Continue => return Ok(ConstEvalFlow::Continue),
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

fn eval_function_tail_expr(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    eval_const_expr_flow(expr, env)
}

fn eval_function_stmt(
    stmt: &EarlyConstStmt,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
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
        EarlyConstStmtKind::Expr(expr) => match eval_const_expr_flow(expr, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
            ConstEvalFlow::Break => Ok(ConstEvalFlow::Break),
            ConstEvalFlow::Continue => Ok(ConstEvalFlow::Continue),
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
        } => eval_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        EarlyConstStmtKind::ForIn(for_in) => eval_for_in_stmt(stmt.span, for_in, env),
        EarlyConstStmtKind::While { cond, body } => eval_while_stmt(stmt.span, cond, body, env),
        EarlyConstStmtKind::Loop { body } => eval_loop_stmt(stmt.span, body, env),
    }
}

fn eval_resolved_function_stmt(
    stmt: &ResolvedConstStmt,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match stmt.kind() {
        ResolvedConstStmtKind::Binding(binding) => {
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
        ResolvedConstStmtKind::Expr(expr) => match eval_resolved_const_expr_flow(expr, env)? {
            ConstEvalFlow::Value(_) | ConstEvalFlow::Void => Ok(ConstEvalFlow::Void),
            ConstEvalFlow::Return(value) => Ok(ConstEvalFlow::Return(value)),
            ConstEvalFlow::Propagate(value) => Ok(ConstEvalFlow::Propagate(value)),
            ConstEvalFlow::Break => Ok(ConstEvalFlow::Break),
            ConstEvalFlow::Continue => Ok(ConstEvalFlow::Continue),
        },
        ResolvedConstStmtKind::Return(value) => {
            let Some(value) = value else {
                return Err(ConstError {
                    span: stmt.span(),
                    message: "const function must return a value".to_string(),
                });
            };
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
        } => eval_resolved_if_stmt(cond, then_branch, else_branch.as_ref(), env),
        ResolvedConstStmtKind::ForIn(for_in) => eval_resolved_for_in_stmt(stmt.span(), for_in, env),
        ResolvedConstStmtKind::While { cond, body } => {
            eval_resolved_while_stmt(stmt.span(), cond, body, env)
        }
        ResolvedConstStmtKind::Loop { body } => eval_resolved_loop_stmt(stmt.span(), body, env),
    }
}

fn eval_assign_expr_flow(
    span: Span,
    assign: &EarlyConstAssign,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let value = match eval_assignment_value_flow(span, assign, env)? {
        ConstEvalFlow::Value(value) => value,
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => return Ok(flow),
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span,
                message: "const assignment requires a value".to_string(),
            });
        }
    };
    let value = assign_target_writeback_value(span, &assign.lhs, value, env)?;
    env.assign_local(span, &assign.lhs, value)?;
    Ok(ConstEvalFlow::Void)
}

fn eval_resolved_assign_expr_flow(
    span: Span,
    assign: &ResolvedConstAssign,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let value = match eval_resolved_assignment_value_flow(span, assign, env)? {
        ConstEvalFlow::Value(value) => value,
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => return Ok(flow),
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span,
                message: "const assignment requires a value".to_string(),
            });
        }
    };
    let value = resolved_assign_target_writeback_value(span, assign.lhs(), value, env)?;
    env.assign_resolved_local(span, assign.lhs(), value)?;
    Ok(ConstEvalFlow::Void)
}

fn eval_assignment_value_flow(
    span: Span,
    assign: &EarlyConstAssign,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let rhs = eval_value_or_return_flow!(&assign.rhs, env);
    if matches!(assign.op, ConstAssignOp::Assign) {
        return Ok(ConstEvalFlow::Value(rhs));
    }
    let lhs = eval_assign_target_value(span, &assign.lhs, env)?;
    let op = assign_op_binary(assign.op).ok_or_else(|| ConstError {
        span,
        message: "unsupported const assignment operator".to_string(),
    })?;
    eval_numeric_binary_value(lhs, op, rhs)
        .map(ConstEvalFlow::Value)
        .map_err(|message| ConstError { span, message })
}

fn eval_resolved_assignment_value_flow(
    span: Span,
    assign: &ResolvedConstAssign,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let rhs = eval_resolved_value_or_return_flow!(assign.rhs(), env);
    if matches!(assign.op(), ConstAssignOp::Assign) {
        return Ok(ConstEvalFlow::Value(rhs));
    }
    let lhs = eval_resolved_assign_target_value(span, assign.lhs(), env)?;
    let op = assign_op_binary(assign.op()).ok_or_else(|| ConstError {
        span,
        message: "unsupported const assignment operator".to_string(),
    })?;
    eval_numeric_binary_value(lhs, op, rhs)
        .map(ConstEvalFlow::Value)
        .map_err(|message| ConstError { span, message })
}

fn eval_assign_target_root_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    match target {
        EarlyConstAssignTarget::Local {
            span: target_span,
            name,
            local_id,
            ..
        } => {
            let Some(local_id) = local_id else {
                return Err(ConstError {
                    span,
                    message: format!(
                        "failed to resolve const assignment target `{}`",
                        env.symbol_name(*name)
                    ),
                });
            };
            env.resolve_name(
                *target_span,
                &EarlyConstName::resolved(*name, ConstNameResolution::Local(*local_id)),
            )
        }
    }
}

fn eval_resolved_assign_target_root_value(
    target: &ResolvedConstAssignTarget,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { span, local_id, .. } => {
            env.resolve_resolved_name(*span, ConstNameResolution::Local(*local_id))
        }
    }
}

fn eval_assign_target_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let value = eval_assign_target_root_value(span, target, env)?;
    match target {
        EarlyConstAssignTarget::Local { path, .. } => {
            eval_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_resolved_assign_target_value(
    span: Span,
    target: &ResolvedConstAssignTarget,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let value = eval_resolved_assign_target_root_value(target, env)?;
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { path, .. } => {
            eval_resolved_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_assign_path_value(
    span: Span,
    mut value: ConstValue,
    path: &[EarlyConstAssignPathElem],
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    for elem in path {
        value = match elem {
            EarlyConstAssignPathElem::Field { span, name } => match value {
                ConstValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ConstError {
                        span: *span,
                        message: format!(
                            "unknown const assignment field `{}`",
                            env.symbol_name(*name)
                        ),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *span,
                        message: "const field assignment requires a struct value".to_string(),
                    });
                }
            },
            EarlyConstAssignPathElem::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_assign_path_index(*elem_span, index, env)?;
                    values.get(index).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("const array assignment index {index} is out of bounds"),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *elem_span,
                        message: "const index assignment requires an array value".to_string(),
                    });
                }
            },
        };
    }
    Ok(value)
}

fn eval_resolved_assign_path_value(
    span: Span,
    mut value: ConstValue,
    path: &[ResolvedConstAssignPathElem],
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    for elem in path {
        value = match elem.kind() {
            ResolvedConstAssignPathElemKind::Field { span, name } => match value {
                ConstValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ConstError {
                        span: *span,
                        message: format!(
                            "unknown const assignment field `{}`",
                            env.symbol_name(*name)
                        ),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *span,
                        message: "const field assignment requires a struct value".to_string(),
                    });
                }
            },
            ResolvedConstAssignPathElemKind::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_resolved_assign_path_index(*elem_span, index, env)?;
                    values.get(index).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("const array assignment index {index} is out of bounds"),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *elem_span,
                        message: "const index assignment requires an array value".to_string(),
                    });
                }
            },
        };
    }
    Ok(value)
}

fn assign_target_writeback_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    match target {
        EarlyConstAssignTarget::Local { path, .. } => {
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
    target: &ResolvedConstAssignTarget,
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { path, .. } => {
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
    root: ConstValue,
    path: &[EarlyConstAssignPathElem],
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        EarlyConstAssignPathElem::Field {
            span: field_span,
            name,
        } => {
            let ConstValue::Struct(mut fields) = root else {
                return Err(ConstError {
                    span: *field_span,
                    message: "const field assignment requires a struct value".to_string(),
                });
            };
            let current = fields.remove(name).ok_or_else(|| ConstError {
                span: *field_span,
                message: format!(
                    "unknown const assignment field `{}`",
                    env.symbol_name(*name)
                ),
            })?;
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            fields.insert(*name, updated);
            Ok(ConstValue::Struct(fields))
        }
        EarlyConstAssignPathElem::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            let index = eval_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn write_resolved_assign_path_value(
    span: Span,
    root: ConstValue,
    path: &[ResolvedConstAssignPathElem],
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head.kind() {
        ResolvedConstAssignPathElemKind::Field {
            span: field_span,
            name,
        } => {
            let ConstValue::Struct(mut fields) = root else {
                return Err(ConstError {
                    span: *field_span,
                    message: "const field assignment requires a struct value".to_string(),
                });
            };
            let current = fields.remove(name).ok_or_else(|| ConstError {
                span: *field_span,
                message: format!(
                    "unknown const assignment field `{}`",
                    env.symbol_name(*name)
                ),
            })?;
            let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
            fields.insert(*name, updated);
            Ok(ConstValue::Struct(fields))
        }
        ResolvedConstAssignPathElemKind::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            let index = eval_resolved_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn eval_assign_path_index(
    span: Span,
    index: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<usize, ConstError> {
    let index_span = index.span;
    let value = match eval_const_expr_flow(index, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => value,
        ConstEvalFlow::Value(_) => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index must be an integer".to_string(),
            });
        }
        ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index cannot contain control flow".to_string(),
            });
        }
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index requires a value".to_string(),
            });
        }
    };
    let index = int_to_array_len(span, value)?;
    usize::try_from(index).map_err(|_| ConstError {
        span,
        message: "const array assignment index is too large".to_string(),
    })
}

fn eval_resolved_assign_path_index(
    span: Span,
    index: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<usize, ConstError> {
    let index_span = index.span();
    let value = match eval_resolved_const_expr_flow(index, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => value,
        ConstEvalFlow::Value(_) => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index must be an integer".to_string(),
            });
        }
        ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index cannot contain control flow".to_string(),
            });
        }
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index requires a value".to_string(),
            });
        }
    };
    let index = int_to_array_len(span, value)?;
    usize::try_from(index).map_err(|_| ConstError {
        span,
        message: "const array assignment index is too large".to_string(),
    })
}

fn assign_op_binary(op: ConstAssignOp) -> Option<ConstBinaryOp> {
    Some(match op {
        ConstAssignOp::Assign => return None,
        ConstAssignOp::Add => ConstBinaryOp::Add,
        ConstAssignOp::Sub => ConstBinaryOp::Sub,
        ConstAssignOp::Shl => ConstBinaryOp::Shl,
        ConstAssignOp::Shr => ConstBinaryOp::Shr,
        ConstAssignOp::Mul => ConstBinaryOp::Mul,
        ConstAssignOp::Div => ConstBinaryOp::Div,
        ConstAssignOp::Rem => ConstBinaryOp::Rem,
        ConstAssignOp::BitAnd => ConstBinaryOp::BitAnd,
        ConstAssignOp::BitXor => ConstBinaryOp::BitXor,
        ConstAssignOp::BitOr => ConstBinaryOp::BitOr,
    })
}

fn eval_range_expr_flow(
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

fn eval_resolved_range_expr_flow(
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
    match eval_const_expr_flow(expr, env)? {
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
    match eval_resolved_const_expr_flow(expr, env)? {
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

fn eval_for_in_stmt(
    span: Span,
    for_in: &EarlyConstForIn,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match eval_const_expr_flow(&for_in.iter, env)? {
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
    let _ = span;
    let _ = &for_in.pattern;
    let _ = &for_in.body;
    let _ = env;
    Err(ConstError {
        span: for_in.iter.span(),
        message: "const for-in Iterator execution is not implemented yet".to_string(),
    })
}

fn eval_resolved_for_in_stmt(
    span: Span,
    for_in: &ResolvedConstForIn,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match eval_resolved_const_expr_flow(for_in.iter(), env)? {
        ConstEvalFlow::Value(_) => {}
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
    }
    let _ = span;
    let _ = for_in.pattern();
    let _ = for_in.body();
    let _ = env;
    Err(ConstError {
        span: for_in.iter().span(),
        message: "const for-in Iterator execution is not implemented yet".to_string(),
    })
}

fn eval_while_stmt(
    span: Span,
    cond: &EarlyConstExpr,
    body: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        let cond_value =
            match eval_condition_flow(cond, env, "const while condition must evaluate to bool")? {
                ConstConditionFlow::Value(value) => value,
                ConstConditionFlow::Flow(flow) => return Ok(flow),
            };
        if !cond_value {
            return Ok(ConstEvalFlow::Void);
        }
        match eval_function_block(body, env)? {
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

fn eval_resolved_while_stmt(
    span: Span,
    cond: &ResolvedConstExpr,
    body: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
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
        match eval_resolved_function_block(body, env)? {
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

fn eval_loop_stmt(
    span: Span,
    body: &EarlyConstBlock,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        match eval_function_block(body, env)? {
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

fn eval_resolved_loop_stmt(
    span: Span,
    body: &ResolvedConstBlock,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    for _ in 0..CONST_LOOP_LIMIT {
        match eval_resolved_function_block(body, env)? {
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

enum ConstConditionFlow {
    Value(bool),
    Flow(ConstEvalFlow),
}

fn eval_if_stmt(
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
        eval_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ConstEvalFlow::Void), |else_branch| {
            eval_function_block(else_branch, env)
        })
    }
}

fn eval_resolved_if_stmt(
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
        eval_resolved_function_block(then_branch, env)
    } else {
        else_branch.map_or(Ok(ConstEvalFlow::Void), |else_branch| {
            eval_resolved_function_block(else_branch, env)
        })
    }
}

fn eval_condition_flow(
    cond: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
    type_error: &'static str,
) -> Result<ConstConditionFlow, ConstError> {
    match eval_const_expr_flow(cond, env)? {
        ConstEvalFlow::Value(ConstValue::Bool(value)) => Ok(ConstConditionFlow::Value(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: cond.span(),
            message: type_error.to_string(),
        }),
        ConstEvalFlow::Return(value) => Ok(ConstConditionFlow::Flow(ConstEvalFlow::Return(value))),
        ConstEvalFlow::Propagate(value) => {
            Ok(ConstConditionFlow::Flow(ConstEvalFlow::Propagate(value)))
        }
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: cond.span(),
            message: "const condition cannot contain loop control flow".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: cond.span(),
            message: "const condition requires a value".to_string(),
        }),
    }
}

fn eval_resolved_condition_flow(
    cond: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
    type_error: &'static str,
) -> Result<ConstConditionFlow, ConstError> {
    match eval_resolved_const_expr_flow(cond, env)? {
        ConstEvalFlow::Value(ConstValue::Bool(value)) => Ok(ConstConditionFlow::Value(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: cond.span(),
            message: type_error.to_string(),
        }),
        ConstEvalFlow::Return(value) => Ok(ConstConditionFlow::Flow(ConstEvalFlow::Return(value))),
        ConstEvalFlow::Propagate(value) => {
            Ok(ConstConditionFlow::Flow(ConstEvalFlow::Propagate(value)))
        }
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span: cond.span(),
            message: "const condition cannot contain loop control flow".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span: cond.span(),
            message: "const condition requires a value".to_string(),
        }),
    }
}

fn const_bit_not(value: IntConst) -> IntConst {
    if value.is_signed() {
        IntConst::from_i128(!value.as_i128().unwrap_or(value.bits() as i128))
    } else {
        IntConst::unsigned(!value.bits())
    }
}

fn int_to_array_len(span: Span, value: IntConst) -> Result<u64, ConstError> {
    let Some(value) = value.as_i128() else {
        return Err(ConstError {
            span,
            message: "array length is too large".to_string(),
        });
    };
    if value < 0 {
        return Err(ConstError {
            span,
            message: "array length must be non-negative".to_string(),
        });
    }
    u64::try_from(value).map_err(|_| ConstError {
        span,
        message: "array length is too large".to_string(),
    })
}

fn int_to_i128(value: IntConst, context: &str) -> Result<i128, String> {
    value
        .as_i128()
        .ok_or_else(|| format!("integer value is too large for {context}"))
}

fn eval_binary_int(lhs: IntConst, op: ConstBinaryOp, rhs: IntConst) -> Result<IntConst, String> {
    if !lhs.is_signed() && !rhs.is_signed() {
        return eval_binary_uint(lhs.bits(), op, rhs.bits()).map(IntConst::unsigned);
    }
    let lhs = int_to_i128(lhs, "const operation")?;
    let rhs = int_to_i128(rhs, "const operation")?;
    Ok(match op {
        ConstBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in const multiplication".to_string())?,
        ConstBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in const expression".to_string());
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| "integer overflow in const division".to_string())?
        }
        ConstBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in const expression".to_string());
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| "integer overflow in const remainder".to_string())?
        }
        ConstBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in const addition".to_string())?,
        ConstBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in const subtraction".to_string())?,
        ConstBinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        ConstBinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        ConstBinaryOp::BitAnd => lhs & rhs,
        ConstBinaryOp::BitXor => lhs ^ rhs,
        ConstBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in const expression: {op:?}"
            ));
        }
    }
    .into())
}

fn eval_binary_uint(lhs: u128, op: ConstBinaryOp, rhs: u128) -> Result<u128, String> {
    Ok(match op {
        ConstBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in const multiplication".to_string())?,
        ConstBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in const expression".to_string());
            }
            lhs / rhs
        }
        ConstBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in const expression".to_string());
            }
            lhs % rhs
        }
        ConstBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in const addition".to_string())?,
        ConstBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in const subtraction".to_string())?,
        ConstBinaryOp::Shl => checked_shift_u128(lhs, rhs, true)?,
        ConstBinaryOp::Shr => checked_shift_u128(lhs, rhs, false)?,
        ConstBinaryOp::BitAnd => lhs & rhs,
        ConstBinaryOp::BitXor => lhs ^ rhs,
        ConstBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in const expression: {op:?}"
            ));
        }
    })
}

fn eval_numeric_binary_value(
    lhs: ConstValue,
    op: ConstBinaryOp,
    rhs: ConstValue,
) -> Result<ConstValue, String> {
    match (lhs, rhs) {
        (ConstValue::Int(lhs), ConstValue::Int(rhs)) => {
            eval_binary_int(lhs, op, rhs).map(ConstValue::Int)
        }
        (ConstValue::Float(lhs), ConstValue::Float(rhs)) => eval_binary_float(lhs, op, rhs),
        _ => Err("const numeric operation requires matching operand types".to_string()),
    }
}

fn eval_numeric_operand_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<Result<ConstValue, ConstEvalFlow>, ConstError> {
    match eval_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value @ (ConstValue::Int(_) | ConstValue::Float(_))) => Ok(Ok(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to a numeric value".to_string(),
        }),
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => Ok(Err(flow)),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const expression requires a value".to_string(),
        }),
    }
}

fn eval_binary_flow(
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
            match (lhs, rhs) {
                (ConstValue::Int(lhs), ConstValue::Int(rhs)) => {
                    ConstValue::Bool(eval_binary_int_compare(lhs, op, rhs))
                }
                (ConstValue::Float(lhs), ConstValue::Float(rhs)) => eval_binary_float(lhs, op, rhs)
                    .map_err(|message| ConstError { span, message })?,
                _ => {
                    return Err(ConstError {
                        span,
                        message: "const comparison requires matching operand types".to_string(),
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
                .map_err(|message| ConstError { span, message })?
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_resolved_numeric_operand_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<Result<ConstValue, ConstEvalFlow>, ConstError> {
    match eval_resolved_const_expr_flow(expr, env)? {
        ConstEvalFlow::Value(value @ (ConstValue::Int(_) | ConstValue::Float(_))) => Ok(Ok(value)),
        ConstEvalFlow::Value(_) => Err(ConstError {
            span: expr.span(),
            message: "const expression must evaluate to a numeric value".to_string(),
        }),
        flow @ (ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue) => Ok(Err(flow)),
        ConstEvalFlow::Void => Err(ConstError {
            span: expr.span(),
            message: "const expression requires a value".to_string(),
        }),
    }
}

fn eval_resolved_binary_flow(
    span: Span,
    lhs: &ResolvedConstExpr,
    op: ConstBinaryOp,
    rhs: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
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
            let lhs = eval_resolved_value_or_return_flow!(lhs, env);
            let rhs = eval_resolved_value_or_return_flow!(rhs, env);
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
                    ConstValue::Bool(eval_binary_int_compare(lhs, op, rhs))
                }
                (ConstValue::Float(lhs), ConstValue::Float(rhs)) => eval_binary_float(lhs, op, rhs)
                    .map_err(|message| ConstError { span, message })?,
                _ => {
                    return Err(ConstError {
                        span,
                        message: "const comparison requires matching operand types".to_string(),
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
                .map_err(|message| ConstError { span, message })?
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_binary_int_compare(lhs: IntConst, op: ConstBinaryOp, rhs: IntConst) -> bool {
    if !lhs.is_signed() && !rhs.is_signed() {
        return match op {
            ConstBinaryOp::Lt => lhs.bits() < rhs.bits(),
            ConstBinaryOp::Le => lhs.bits() <= rhs.bits(),
            ConstBinaryOp::Gt => lhs.bits() > rhs.bits(),
            ConstBinaryOp::Ge => lhs.bits() >= rhs.bits(),
            _ => unreachable!("non-comparison binary operator routed to integer comparison"),
        };
    }
    let lhs = lhs.as_i128().unwrap_or(i128::MAX);
    let rhs = rhs.as_i128().unwrap_or(i128::MAX);
    match op {
        ConstBinaryOp::Lt => lhs < rhs,
        ConstBinaryOp::Le => lhs <= rhs,
        ConstBinaryOp::Gt => lhs > rhs,
        ConstBinaryOp::Ge => lhs >= rhs,
        _ => unreachable!("non-comparison binary operator routed to integer comparison"),
    }
}

fn eval_binary_float(lhs: f64, op: ConstBinaryOp, rhs: f64) -> Result<ConstValue, String> {
    Ok(match op {
        ConstBinaryOp::Add => ConstValue::Float(lhs + rhs),
        ConstBinaryOp::Sub => ConstValue::Float(lhs - rhs),
        ConstBinaryOp::Mul => ConstValue::Float(lhs * rhs),
        ConstBinaryOp::Div => ConstValue::Float(lhs / rhs),
        ConstBinaryOp::Rem => ConstValue::Float(lhs % rhs),
        ConstBinaryOp::Lt => ConstValue::Bool(lhs < rhs),
        ConstBinaryOp::Le => ConstValue::Bool(lhs <= rhs),
        ConstBinaryOp::Gt => ConstValue::Bool(lhs > rhs),
        ConstBinaryOp::Ge => ConstValue::Bool(lhs >= rhs),
        _ => {
            return Err(format!(
                "unsupported binary operator for float const expression: {op:?}"
            ));
        }
    })
}

fn values_equal(lhs: &ConstValue, rhs: &ConstValue) -> Option<bool> {
    match (lhs, rhs) {
        (ConstValue::Int(lhs), ConstValue::Int(rhs)) => Some(lhs == rhs),
        (ConstValue::Float(lhs), ConstValue::Float(rhs)) => Some(lhs == rhs),
        (ConstValue::Bool(lhs), ConstValue::Bool(rhs)) => Some(lhs == rhs),
        (ConstValue::String(lhs), ConstValue::String(rhs)) => Some(lhs == rhs),
        (ConstValue::Pointer(lhs), ConstValue::Pointer(rhs)) => values_equal(lhs, rhs),
        (ConstValue::Pointer(lhs), rhs) => values_equal(lhs, rhs),
        (lhs, ConstValue::Pointer(rhs)) => values_equal(lhs, rhs),
        (ConstValue::String(lhs), ConstValue::Array(rhs)) => {
            Some(char_array_to_string(rhs)? == *lhs)
        }
        (ConstValue::Array(lhs), ConstValue::String(rhs)) => {
            Some(char_array_to_string(lhs)? == *rhs)
        }
        (ConstValue::Range(lhs), ConstValue::Range(rhs)) => Some(lhs == rhs),
        (ConstValue::Array(lhs), ConstValue::Array(rhs)) => {
            if lhs.len() != rhs.len() {
                return Some(false);
            }
            lhs.iter()
                .zip(rhs)
                .try_fold(true, |_, (lhs, rhs)| values_equal(lhs, rhs))
        }
        (ConstValue::Optional(lhs), ConstValue::Optional(rhs)) => match (lhs, rhs) {
            (None, None) => Some(true),
            (Some(lhs), Some(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        (ConstValue::ErrorUnion(lhs), ConstValue::ErrorUnion(rhs)) => match (lhs, rhs) {
            (Ok(lhs), Ok(rhs)) | (Err(lhs), Err(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        _ => None,
    }
}
