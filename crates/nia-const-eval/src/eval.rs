use crate::{
    ConstCommonEnv, ConstEnumPayload, ConstError, ConstPointerPathElem, ConstPointerValue,
    ConstRangeValue, ConstValue, EarlyConstEnv, ResolvedConstCallOutput, ResolvedConstEnv,
    ResolvedConstPlace, ResolvedConstPlaceElem,
    literals::{
        bytes_to_array, const_error_message, decode_byte_char_literal, decode_char_literal,
        eval_byte_string_literal, eval_float_literal, eval_int_literal, eval_string_literal,
        string_to_char_array,
    },
    numeric::{
        const_bit_not, const_typed_bit_not, eval_binary_float, eval_binary_int,
        eval_binary_int_compare, eval_numeric_binary_value, eval_typed_binary_int,
        eval_typed_int_neg, int_to_array_len, values_equal,
    },
};

use nia_const_ir::{
    ConstAssignOp, ConstBinaryOp, ConstEnumPatternFields, ConstNameResolution, ConstUnaryOp,
    EarlyConstArrayElements, EarlyConstAssign, EarlyConstAssignPathElem, EarlyConstAssignTarget,
    EarlyConstBlock, EarlyConstExpr, EarlyConstExprKind, EarlyConstForIn, EarlyConstFunction,
    EarlyConstMatch, EarlyConstMatchArm, EarlyConstMatchArmBody, EarlyConstName, EarlyConstParam,
    EarlyConstPattern, EarlyConstPatternBinding, EarlyConstRange, EarlyConstSliceRange,
    EarlyConstStmt, EarlyConstStmtKind, ResolvedConstArrayElements, ResolvedConstArrayElementsKind,
    ResolvedConstAssign, ResolvedConstAssignPathElem, ResolvedConstAssignPathElemKind,
    ResolvedConstAssignTarget, ResolvedConstAssignTargetKind, ResolvedConstBlock,
    ResolvedConstExpr, ResolvedConstExprKind, ResolvedConstFieldInit, ResolvedConstForIn,
    ResolvedConstFunction, ResolvedConstMatch, ResolvedConstMatchArm, ResolvedConstMatchArmBody,
    ResolvedConstMatchArmBodyKind, ResolvedConstParam, ResolvedConstPatternBinding,
    ResolvedConstPatternKind, ResolvedConstRange, ResolvedConstSliceRange, ResolvedConstStmt,
    ResolvedConstStmtKind,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
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

fn with_const_eval_session<E, T>(
    env: &mut E,
    evaluate: impl FnOnce(&mut E) -> Result<T, ConstError>,
) -> Result<T, ConstError>
where
    E: ConstCommonEnv + ?Sized,
{
    env.begin_const_eval();
    let result = evaluate(env);
    env.end_const_eval();
    result
}

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

struct ConstMatchResult<'a> {
    arm: &'a EarlyConstMatchArm,
    bindings: Vec<ConstMatchBinding>,
}

struct ConstMatchBinding {
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
    with_const_eval_session(env, |env| {
        let value = eval_const_expr(expr, env)?;
        env.validate_const_root_result(expr.span(), &value)?;
        Ok(value)
    })
}

pub fn eval_resolved_const_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    with_const_eval_session(env, |env| {
        let value = eval_resolved_const_expr_value(expr, env)?;
        env.validate_const_root_result(expr.span(), &value)?;
        Ok(value)
    })
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

mod aggregates;
mod arrays;
mod assignment;
mod binary;
mod blocks;
mod calls;
mod control_flow;
mod patterns;
mod ranges;
pub use assignment::write_resolved_const_place;
use blocks::{
    eval_function_block, eval_function_stmt, eval_function_tail_expr, eval_resolved_function_block,
    eval_resolved_function_stmt, eval_resolved_function_tail_expr,
};
pub use calls::{
    ResolvedConstCallInput, eval_early_const_function_call, eval_resolved_const_function_call,
};

fn eval_resolved_const_expr_flow(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let span = expr.span();
    env.consume_const_eval_step(span)?;
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
        ResolvedConstExprKind::Name(resolution) => match resolution {
            ConstNameResolution::Global(variant) if env.is_enum_variant(*variant) => {
                ConstValue::Enum {
                    variant: *variant,
                    payload: ConstEnumPayload::Unit,
                }
            }
            _ => env.resolve_resolved_name(span, resolution.clone())?,
        },
        ResolvedConstExprKind::Field { lhs, name } => {
            match eval_resolved_value_or_return_flow!(lhs, env) {
                ConstValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("unknown const field `{}`", env.symbol_name(*name)),
                    })?
                }
                ConstValue::Union(value) => value.read(*name).map_err(|message| ConstError {
                    span,
                    message: format!("{message} `{}`", env.symbol_name(*name)),
                })?,
                _ => {
                    return Err(ConstError {
                        span,
                        message: "const field access requires a struct value".to_string(),
                    });
                }
            }
        }
        ResolvedConstExprKind::Index { lhs, index } => {
            return arrays::eval_resolved_array_index_flow(span, lhs, index, env);
        }
        ResolvedConstExprKind::Slice { lhs, range } => {
            return arrays::eval_resolved_array_slice_flow(span, lhs, range, env);
        }
        ResolvedConstExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_resolved_value_or_return_flow!(elem, env));
            }
            ConstValue::Tuple(values)
        }
        ResolvedConstExprKind::TupleField { lhs, index } => {
            let value = eval_resolved_value_or_return_flow!(lhs, env);
            let ConstValue::Tuple(elems) = value else {
                return Err(ConstError {
                    span,
                    message: "const tuple projection requires a tuple value".to_string(),
                });
            };
            elems.get(*index).cloned().ok_or_else(|| ConstError {
                span,
                message: format!("const tuple field index {index} is out of bounds"),
            })?
        }
        ResolvedConstExprKind::ArrayLiteral { elems, .. } => {
            return arrays::eval_resolved_array_literal_flow(elems, env);
        }
        ResolvedConstExprKind::StructLiteral { ty, fields } => {
            return aggregates::eval_resolved_struct_literal_flow(span, *ty, fields, env);
        }
        ResolvedConstExprKind::TupleStructLiteral { fields, .. } => {
            let mut values = std::collections::BTreeMap::new();
            for field in fields {
                values.insert(
                    *field.name_symbol(),
                    eval_resolved_value_or_return_flow!(field.value(), env),
                );
            }
            return Ok(ConstEvalFlow::Value(ConstValue::Struct(values)));
        }
        ResolvedConstExprKind::EnumStructLiteral { variant, fields } => {
            return aggregates::eval_resolved_enum_struct_literal_flow(span, variant, fields, env);
        }
        ResolvedConstExprKind::CompileError { message } => {
            let value = eval_resolved_value_or_return_flow!(message, env);
            let Some(message) = const_error_message_from_value(span, &value, env)? else {
                return Err(ConstError {
                    span,
                    message: "builtin `error` requires a const string message".to_string(),
                });
            };
            return Err(ConstError { span, message });
        }
        ResolvedConstExprKind::Trap => {
            return Err(ConstError {
                span,
                message: "builtin `trap` reached during const evaluation".to_string(),
            });
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
            generic_args,
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
                env.prepare_resolved_call_arguments(span, callee, generic_args, args)?;
                let has_receiver = matches!(callee.kind(), ResolvedConstExprKind::Method { .. });
                let mut values = Vec::with_capacity(args.len() + usize::from(has_receiver));
                let mut receiver_place = None;
                if let ResolvedConstExprKind::Method { receiver, .. } = callee.kind() {
                    match eval_resolved_call_receiver(receiver, env)? {
                        ResolvedConstCallReceiver::Place { value, place } => {
                            values.push(value);
                            receiver_place = Some(place);
                        }
                        ResolvedConstCallReceiver::Value(value) => values.push(value),
                        ResolvedConstCallReceiver::Flow(flow) => return Ok(flow),
                    }
                }
                for arg in args {
                    values.push(eval_resolved_value_or_return_flow!(arg, env));
                }
                if let Some(variant) = aggregates::resolved_enum_variant_id(callee, env) {
                    ConstValue::Enum {
                        variant,
                        payload: ConstEnumPayload::Tuple(values),
                    }
                } else {
                    env.call_resolved_function(
                        span,
                        callee,
                        generic_args,
                        args,
                        receiver_place.as_ref(),
                        values,
                    )?
                }
            }
        }
        ResolvedConstExprKind::Unary {
            op: ConstUnaryOp::Neg,
            expr: inner,
        } => match eval_resolved_value_or_return_flow!(inner, env) {
            ConstValue::Int(value) => ConstValue::Int(
                eval_typed_int_neg(value, env.resolved_integer_semantics(expr))
                    .map_err(|message| ConstError { span, message })?,
            ),
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
            ConstValue::Int(value) => ConstValue::Int(const_typed_bit_not(
                value,
                env.resolved_integer_semantics(expr),
            )),
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
            ConstValue::Pointer(pointer) => env.dereference_const_pointer(span, &pointer)?,
            _ => {
                return Err(ConstError {
                    span,
                    message: "const dereference requires a pointer value".to_string(),
                });
            }
        },
        ResolvedConstExprKind::Unary {
            op: op @ (ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref),
            expr: inner,
        } => match eval_resolved_call_receiver(inner, env)? {
            ResolvedConstCallReceiver::Place { value, place } => env.reference_resolved_place(
                span,
                &place,
                value,
                matches!(op, ConstUnaryOp::RefReadOnly),
            )?,
            ResolvedConstCallReceiver::Value(value) => {
                env.reference_const_value(span, value, matches!(op, ConstUnaryOp::RefReadOnly))?
            }
            ResolvedConstCallReceiver::Flow(flow) => return Ok(flow),
        },
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
            return binary::eval_resolved_binary_flow(expr, lhs, *op, rhs, env);
        }
        ResolvedConstExprKind::Assign(assign) => {
            env.prepare_resolved_assignment(assign)?;
            return assignment::eval_resolved_assign_expr_flow(span, assign, env);
        }
        ResolvedConstExprKind::Range(range) => {
            return ranges::eval_resolved_range_expr_flow(range, env);
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
        ResolvedConstExprKind::Match(matched) => {
            return eval_resolved_const_match_expr_flow(matched, env);
        }
        ResolvedConstExprKind::Cast { expr: inner, ty } => {
            let value = eval_resolved_value_or_return_flow!(inner, env);
            env.cast_value(span, value, *ty)?
        }
        ResolvedConstExprKind::Block(block) => {
            return eval_resolved_function_block(block, env);
        }
        ResolvedConstExprKind::Method { .. } | ResolvedConstExprKind::AssociatedFunction { .. } => {
            return Err(ConstError {
                span,
                message: "const function target cannot be used as a value".to_string(),
            });
        }
    };
    Ok(ConstEvalFlow::Value(value))
}

fn eval_const_expr_flow(
    expr: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    env.consume_const_eval_step(expr.span())?;
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
            if let Some(variant) = aggregates::early_enum_variant_id(name, env) {
                ConstValue::Enum {
                    variant,
                    payload: ConstEnumPayload::Unit,
                }
            } else {
                env.resolve_name(expr.span, name)?
            }
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
        EarlyConstExprKind::Index { lhs, index } => {
            return arrays::eval_array_index_flow(expr.span, lhs, index, env);
        }
        EarlyConstExprKind::Slice { lhs, range } => {
            return arrays::eval_array_slice_flow(expr.span, lhs, range, env);
        }
        EarlyConstExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                values.push(eval_value_or_return_flow!(elem, env));
            }
            ConstValue::Tuple(values)
        }
        EarlyConstExprKind::TupleField { lhs, index } => {
            let value = eval_value_or_return_flow!(lhs, env);
            let ConstValue::Tuple(elems) = value else {
                return Err(ConstError {
                    span: expr.span,
                    message: "const tuple projection requires a tuple value".to_string(),
                });
            };
            elems.get(*index).cloned().ok_or_else(|| ConstError {
                span: expr.span,
                message: format!("const tuple field index {index} is out of bounds"),
            })?
        }
        EarlyConstExprKind::ArrayLiteral { elems, .. } => {
            return arrays::eval_array_literal_flow(elems, env);
        }
        EarlyConstExprKind::StructLiteral { fields, .. } => {
            return aggregates::eval_struct_literal_flow(fields, env);
        }
        EarlyConstExprKind::TupleStructLiteral { fields, .. } => {
            let mut values = std::collections::BTreeMap::new();
            for field in fields {
                values.insert(field.name, eval_value_or_return_flow!(&field.value, env));
            }
            return Ok(ConstEvalFlow::Value(ConstValue::Struct(values)));
        }
        EarlyConstExprKind::EnumStructLiteral { variant, fields } => {
            return aggregates::eval_enum_struct_literal_flow(expr.span, variant, fields, env);
        }
        EarlyConstExprKind::CompileError { message } => {
            let value = eval_value_or_return_flow!(message, env);
            let Some(message) = const_error_message_from_value(expr.span, &value, env)? else {
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
        EarlyConstExprKind::Trap => {
            return Err(ConstError {
                span: expr.span,
                message: "builtin `trap` reached during const evaluation".to_string(),
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
            generic_args,
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
                if let Some(variant) = aggregates::early_enum_expr_variant_id(callee, env) {
                    ConstValue::Enum {
                        variant,
                        payload: ConstEnumPayload::Tuple(values),
                    }
                } else {
                    env.call_function(expr.span, callee, generic_args, args, values)?
                }
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
            ConstValue::Pointer(pointer) => env.dereference_const_pointer(expr.span, &pointer)?,
            _ => {
                return Err(ConstError {
                    span: expr.span,
                    message: "const dereference requires a pointer value".to_string(),
                });
            }
        },
        EarlyConstExprKind::Unary {
            op: op @ (ConstUnaryOp::RefReadOnly | ConstUnaryOp::Ref),
            expr: inner,
        } => {
            let value = eval_value_or_return_flow!(inner, env);
            env.reference_const_value(expr.span, value, matches!(op, ConstUnaryOp::RefReadOnly))?
        }
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
            return binary::eval_binary_flow(expr.span, lhs, *op, rhs, env);
        }
        EarlyConstExprKind::Assign(assign) => {
            return assignment::eval_assign_expr_flow(expr.span, assign, env);
        }
        EarlyConstExprKind::Range(range) => {
            return ranges::eval_range_expr_flow(range, env);
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
        EarlyConstExprKind::Match(matched) => {
            return eval_const_match_expr_flow(matched, env);
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
        EarlyConstExprKind::Method { .. } | EarlyConstExprKind::AssociatedFunction { .. } => {
            return Err(ConstError {
                span: expr.span,
                message: "const function target cannot be used as a value".to_string(),
            });
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
    let cond_value = match control_flow::eval_condition_flow(
        cond,
        env,
        "const expression must evaluate to bool",
    )? {
        control_flow::ConstConditionFlow::Value(value) => value,
        control_flow::ConstConditionFlow::Flow(flow) => return Ok(flow),
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

enum ResolvedConstCallReceiver {
    Place {
        value: ConstValue,
        place: ResolvedConstPlace,
    },
    Value(ConstValue),
    Flow(ConstEvalFlow),
}

fn eval_resolved_call_receiver(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<ResolvedConstCallReceiver, ConstError> {
    match expr.kind() {
        ResolvedConstExprKind::Name(ConstNameResolution::Local(local_id)) => {
            let value =
                env.resolve_resolved_name(expr.span(), ConstNameResolution::Local(*local_id))?;
            Ok(ResolvedConstCallReceiver::Place {
                value,
                place: ResolvedConstPlace {
                    local_id: *local_id,
                    path: Vec::new(),
                },
            })
        }
        ResolvedConstExprKind::Field { lhs, name } => {
            let receiver = eval_resolved_call_receiver(lhs, env)?;
            let (value, mut place) = match receiver {
                ResolvedConstCallReceiver::Place { value, place } => (value, Some(place)),
                ResolvedConstCallReceiver::Value(value) => (value, None),
                ResolvedConstCallReceiver::Flow(flow) => {
                    return Ok(ResolvedConstCallReceiver::Flow(flow));
                }
            };
            if let Some(place) = &mut place {
                place.path.push(ResolvedConstPlaceElem::Field(*name));
            }
            let ConstValue::Struct(fields) = value else {
                return Err(ConstError {
                    span: expr.span(),
                    message: "const field access requires a struct value".to_string(),
                });
            };
            let value = fields.get(name).cloned().ok_or_else(|| ConstError {
                span: expr.span(),
                message: format!("unknown const field `{}`", env.symbol_name(*name)),
            })?;
            Ok(match place {
                Some(place) => ResolvedConstCallReceiver::Place { value, place },
                None => ResolvedConstCallReceiver::Value(value),
            })
        }
        ResolvedConstExprKind::Index { lhs, index } => {
            let receiver = eval_resolved_call_receiver(lhs, env)?;
            let (value, mut place) = match receiver {
                ResolvedConstCallReceiver::Place { value, place } => (value, Some(place)),
                ResolvedConstCallReceiver::Value(value) => (value, None),
                ResolvedConstCallReceiver::Flow(flow) => {
                    return Ok(ResolvedConstCallReceiver::Flow(flow));
                }
            };
            let index = assignment::eval_resolved_assign_path_index(index.span(), index, env)?;
            if let Some(place) = &mut place {
                place.path.push(ResolvedConstPlaceElem::Index(index));
            }
            let ConstValue::Array(values) = value else {
                return Err(ConstError {
                    span: expr.span(),
                    message: "const index access requires an array value".to_string(),
                });
            };
            let value = values.get(index).cloned().ok_or_else(|| ConstError {
                span: expr.span(),
                message: format!("const array index {index} is out of bounds"),
            })?;
            Ok(match place {
                Some(place) => ResolvedConstCallReceiver::Place { value, place },
                None => ResolvedConstCallReceiver::Value(value),
            })
        }
        _ => match eval_resolved_const_expr_flow(expr, env)? {
            ConstEvalFlow::Value(value) => Ok(ResolvedConstCallReceiver::Value(value)),
            ConstEvalFlow::Void => Err(ConstError {
                span: expr.span(),
                message: "const method receiver requires a value".to_string(),
            }),
            flow => Ok(ResolvedConstCallReceiver::Flow(flow)),
        },
    }
}

fn const_error_message_from_value(
    span: Span,
    value: &ConstValue,
    env: &mut impl ConstCommonEnv,
) -> Result<Option<String>, ConstError> {
    if let ConstValue::Pointer(pointer) = value {
        let value = env.dereference_const_pointer(span, pointer)?;
        return const_error_message_from_value(span, &value, env);
    }
    Ok(const_error_message(value))
}

pub fn eval_const_range_bound_value(
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

pub fn eval_const_slice_pointer_value(
    span: Span,
    value: ConstValue,
    mutable: bool,
) -> Result<ConstValue, ConstError> {
    let ConstValue::Pointer(pointer) = value else {
        return Err(ConstError {
            span,
            message: "const slice pointer method requires a slice pointer".to_string(),
        });
    };
    match pointer {
        ConstPointerValue::Frozen {
            origin,
            is_readonly,
            pointee,
        } => {
            if mutable && is_readonly {
                return Err(ConstError {
                    span,
                    message: "const mutable slice pointer requires a mutable slice".to_string(),
                });
            }
            let ConstValue::Array(values) = *pointee else {
                return Err(ConstError {
                    span,
                    message: "const slice pointer method requires an array-backed slice"
                        .to_string(),
                });
            };
            let Some(first) = values.into_iter().next() else {
                return Err(ConstError {
                    span,
                    message: "const slice pointer method cannot project an empty slice".to_string(),
                });
            };
            Ok(ConstValue::Pointer(ConstPointerValue::Frozen {
                origin,
                is_readonly: !mutable,
                pointee: Box::new(first),
            }))
        }
        ConstPointerValue::Place {
            allocation,
            mut path,
        } => {
            path.push(ConstPointerPathElem::Index(0));
            Ok(ConstValue::Pointer(ConstPointerValue::Place {
                allocation,
                path,
            }))
        }
    }
}

fn eval_const_match_expr_flow(
    matched: &EarlyConstMatch,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let target = eval_value_or_return_flow!(&matched.target, env);
    let Some(matched) = matching_match_arm(&target, matched, env)? else {
        return Err(ConstError {
            span: matched.span,
            message: "const match expression did not match any arm".to_string(),
        });
    };
    eval_const_match_match_body(matched, env)
}

fn eval_resolved_const_if_expr_flow(
    span: Span,
    cond: &ResolvedConstExpr,
    then_branch: &ResolvedConstBlock,
    else_branch: Option<&ResolvedConstExpr>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let cond_value = match control_flow::eval_resolved_condition_flow(
        cond,
        env,
        "const expression must evaluate to bool",
    )? {
        control_flow::ConstConditionFlow::Value(value) => value,
        control_flow::ConstConditionFlow::Flow(flow) => return Ok(flow),
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

struct ResolvedConstMatchResult<'a> {
    arm: &'a ResolvedConstMatchArm,
    bindings: Vec<ConstMatchBinding>,
}

fn eval_resolved_const_match_expr_flow(
    matched: &ResolvedConstMatch,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let target = eval_resolved_value_or_return_flow!(matched.target(), env);
    let Some(matched) = matching_resolved_match_arm(&target, matched, env)? else {
        return Err(ConstError {
            span: matched.span(),
            message: "const match expression did not match any arm".to_string(),
        });
    };
    eval_resolved_const_match_match_body(matched, env)
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
    env.prepare_resolved_try(span, inner)?;
    match eval_resolved_const_expr_flow(inner, env)? {
        ConstEvalFlow::Value(ConstValue::Optional(Some(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::Optional(None)) => {
            Ok(ConstEvalFlow::Propagate(ConstValue::Optional(None)))
        }
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Ok(value))) => Ok(ConstEvalFlow::Value(*value)),
        ConstEvalFlow::Value(ConstValue::ErrorUnion(Err(value))) => {
            let value = env.convert_resolved_try_error(span, *value)?;
            Ok(ConstEvalFlow::Propagate(ConstValue::ErrorUnion(Err(
                Box::new(value),
            ))))
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

fn matching_match_arm<'a>(
    target: &ConstValue,
    matched: &'a EarlyConstMatch,
    env: &mut impl EarlyConstEnv,
) -> Result<Option<ConstMatchResult<'a>>, ConstError> {
    let mut default = None;
    for arm in &matched.arms {
        for pattern in &arm.patterns {
            if matches!(pattern, EarlyConstPattern::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if patterns::early_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ConstMatchResult { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ConstMatchResult {
        arm,
        bindings: Vec::new(),
    }))
}

fn matching_resolved_match_arm<'a>(
    target: &ConstValue,
    matched: &'a ResolvedConstMatch,
    env: &mut impl ResolvedConstEnv,
) -> Result<Option<ResolvedConstMatchResult<'a>>, ConstError> {
    let mut default = None;
    for arm in matched.arms() {
        for pattern in arm.patterns() {
            if matches!(pattern.kind(), ResolvedConstPatternKind::Wildcard { .. }) {
                default = Some(arm);
                continue;
            }
            let mut bindings = Vec::new();
            if patterns::resolved_pattern_matches(target, pattern, env, &mut bindings)? {
                return Ok(Some(ResolvedConstMatchResult { arm, bindings }));
            }
        }
    }
    Ok(default.map(|arm| ResolvedConstMatchResult {
        arm,
        bindings: Vec::new(),
    }))
}

fn eval_const_match_arm_body(
    body: &EarlyConstMatchArmBody,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match body {
        EarlyConstMatchArmBody::Expr(expr) => eval_function_tail_expr(expr, env),
        EarlyConstMatchArmBody::Stmt(stmt) => eval_function_stmt(stmt, env),
        EarlyConstMatchArmBody::Block(block) => eval_function_block(block, env),
    }
}

fn eval_const_match_match_body(
    matched: ConstMatchResult<'_>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if matched.bindings.is_empty() {
        return eval_const_match_arm_body(&matched.arm.body, env);
    }
    env.push_const_scope(matched.arm.span)?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| patterns::bind_pattern_value(binding, env));
    let result = bind_result.and_then(|()| eval_const_match_arm_body(&matched.arm.body, env));
    env.pop_const_scope();
    result
}

fn eval_resolved_const_match_arm_body(
    body: &ResolvedConstMatchArmBody,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    match body.kind() {
        ResolvedConstMatchArmBodyKind::Expr(expr) => eval_resolved_function_tail_expr(expr, env),
        ResolvedConstMatchArmBodyKind::Stmt(stmt) => eval_resolved_function_stmt(stmt, env),
        ResolvedConstMatchArmBodyKind::Block(block) => eval_resolved_function_block(block, env),
    }
}

fn eval_resolved_const_match_match_body(
    matched: ResolvedConstMatchResult<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    if matched.bindings.is_empty() {
        return eval_resolved_const_match_arm_body(matched.arm.body(), env);
    }
    env.push_const_scope(matched.arm.span())?;
    let bind_result = matched
        .bindings
        .iter()
        .try_for_each(|binding| patterns::bind_resolved_pattern_value(binding, env));
    let result =
        bind_result.and_then(|()| eval_resolved_const_match_arm_body(matched.arm.body(), env));
    env.pop_const_scope();
    result
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
    with_const_eval_session(env, |env| eval_const_int_expr(expr, env))
}

pub fn eval_resolved_const_int_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<IntConst, ConstError> {
    with_const_eval_session(env, |env| eval_resolved_const_int_expr_inner(expr, env))
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
    with_const_eval_session(env, |env| eval_const_bool_expr(expr, env))
}

pub fn eval_resolved_const_bool_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<bool, ConstError> {
    with_const_eval_session(env, |env| eval_resolved_const_bool_expr_inner(expr, env))
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
    with_const_eval_session(env, |env| eval_const_array_len_expr(expr, env))
}

pub fn eval_resolved_const_array_len_expr(
    expr: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<u64, ConstError> {
    with_const_eval_session(env, |env| {
        int_to_array_len(expr.span(), eval_resolved_const_int_expr_inner(expr, env)?)
    })
}
