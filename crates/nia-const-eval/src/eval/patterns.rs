// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn early_pattern_matches(
    target: &ConstValue,
    pattern: &EarlyConstPattern,
    env: &mut impl EarlyConstEnv,
    bindings: &mut Vec<ConstSwitchBinding>,
) -> Result<bool, ConstError> {
    // Bindings are appended only as a pattern branch succeeds. The caller
    // discards this vector when a pattern returns false, so failed alternatives
    // cannot leak locals into the selected arm.
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
            ConstValue::Pointer(pointer) => {
                let value = env.dereference_const_pointer(*span, pointer)?;
                early_pattern_matches(&value, pattern, env, bindings)
            }
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
        EarlyConstPattern::Tuple { patterns, span } => tuple_pattern_matches(
            target,
            patterns,
            bindings,
            *span,
            |value, pattern, bindings| early_pattern_matches(value, pattern, env, bindings),
        ),
        EarlyConstPattern::EnumVariant {
            variant,
            fields,
            span,
        } => {
            let Some(variant) = super::aggregates::early_enum_expr_variant_id(variant, env) else {
                return Err(ConstError {
                    span: *span,
                    message: "const enum pattern requires a resolved enum variant".to_string(),
                });
            };
            enum_pattern_matches(
                target,
                variant,
                fields,
                bindings,
                *span,
                |value, pattern, bindings| early_pattern_matches(value, pattern, env, bindings),
            )
        }
        EarlyConstPattern::Struct { fields, span, .. } => struct_pattern_matches(
            target,
            fields,
            bindings,
            *span,
            |value, pattern, bindings| early_pattern_matches(value, pattern, env, bindings),
        ),
        EarlyConstPattern::Expr(pattern) => {
            let pattern = super::eval_const_expr(pattern, env)?;
            Ok(super::values_equal(target, &pattern).unwrap_or(false))
        }
        EarlyConstPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

pub(super) fn resolved_pattern_matches(
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
            ConstValue::Pointer(pointer) => {
                let value = env.dereference_const_pointer(*span, pointer)?;
                resolved_pattern_matches(&value, pattern, env, bindings)
            }
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
        ResolvedConstPatternKind::Tuple { patterns, span } => tuple_pattern_matches(
            target,
            patterns,
            bindings,
            *span,
            |value, pattern, bindings| resolved_pattern_matches(value, pattern, env, bindings),
        ),
        ResolvedConstPatternKind::EnumVariant {
            variant,
            fields,
            span,
        } => {
            let Some(variant) = super::aggregates::resolved_enum_variant_id(variant, env) else {
                return Err(ConstError {
                    span: *span,
                    message: "const enum pattern requires a resolved enum variant".to_string(),
                });
            };
            enum_pattern_matches(
                target,
                variant,
                fields,
                bindings,
                *span,
                |value, pattern, bindings| resolved_pattern_matches(value, pattern, env, bindings),
            )
        }
        ResolvedConstPatternKind::Struct { fields, span, .. } => struct_pattern_matches(
            target,
            fields,
            bindings,
            *span,
            |value, pattern, bindings| resolved_pattern_matches(value, pattern, env, bindings),
        ),
        ResolvedConstPatternKind::Expr(pattern) => {
            let pattern = super::eval_resolved_const_expr_value(pattern, env)?;
            Ok(super::values_equal(target, &pattern).unwrap_or(false))
        }
        ResolvedConstPatternKind::Range {
            start,
            end,
            inclusive,
            span,
        } => resolved_switch_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn struct_pattern_matches<P>(
    target: &ConstValue,
    fields: &[nia_const_ir::ConstNamedPatternField<P>],
    bindings: &mut Vec<ConstSwitchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstSwitchBinding>,
    ) -> Result<bool, ConstError>,
) -> Result<bool, ConstError> {
    // Constructor identity and the complete field set are type-checking invariants. Evaluation
    // only performs the recursive value match, using names because ConstValue stores struct fields
    // by symbol rather than declaration index.
    let ConstValue::Struct(values) = target else {
        return Err(ConstError {
            span,
            message: "const struct pattern requires a struct target".to_string(),
        });
    };
    for field in fields {
        let Some(value) = values.get(&field.name) else {
            return Err(ConstError {
                span: field.span,
                message: "const struct pattern references a missing field".to_string(),
            });
        };
        if !pattern_matches(value, &field.pattern, bindings)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn enum_pattern_matches<P>(
    target: &ConstValue,
    variant: GlobalDefId,
    fields: &ConstEnumPatternFields<P>,
    bindings: &mut Vec<ConstSwitchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstSwitchBinding>,
    ) -> Result<bool, ConstError>,
) -> Result<bool, ConstError> {
    let ConstValue::Enum {
        variant: actual_variant,
        payload,
    } = target
    else {
        return Err(ConstError {
            span,
            message: "const enum pattern requires an enum target".to_string(),
        });
    };
    if *actual_variant != variant {
        return Ok(false);
    }
    match (fields, payload) {
        (ConstEnumPatternFields::Tuple(patterns), ConstEnumPayload::Tuple(values)) => {
            if patterns.len() != values.len() {
                return Err(ConstError {
                    span,
                    message: "const enum tuple pattern has the wrong arity".to_string(),
                });
            }
            for (pattern, value) in patterns.iter().zip(values) {
                if !pattern_matches(value, pattern, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (ConstEnumPatternFields::Named(patterns), ConstEnumPayload::Named(values)) => {
            for field in patterns {
                let Some(value) = values.get(&field.name) else {
                    return Err(ConstError {
                        span: field.span,
                        message: "const enum named pattern references a missing field".to_string(),
                    });
                };
                if !pattern_matches(value, &field.pattern, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Err(ConstError {
            span,
            message: "const enum pattern shape does not match its variant payload".to_string(),
        }),
    }
}

fn tuple_pattern_matches<P>(
    target: &ConstValue,
    patterns: &[P],
    bindings: &mut Vec<ConstSwitchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstSwitchBinding>,
    ) -> Result<bool, ConstError>,
) -> Result<bool, ConstError> {
    let ConstValue::Tuple(values) = target else {
        return Err(ConstError {
            span,
            message: "const tuple pattern requires a tuple target".to_string(),
        });
    };
    if values.len() != patterns.len() {
        return Ok(false);
    }
    for (value, pattern) in values.iter().zip(patterns) {
        if !pattern_matches(value, pattern, bindings)? {
            return Ok(false);
        }
    }
    Ok(true)
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
    let start = super::eval_const_int_expr(start, env)?;
    let end = super::eval_const_int_expr(end, env)?;
    // Keep range matching in integer space and choose the comparison operator
    // explicitly: `a..b` excludes `b`, while `a..=b` includes it.
    Ok(if inclusive {
        super::eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && super::eval_binary_int_compare(*target, ConstBinaryOp::Le, end)
    } else {
        super::eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && super::eval_binary_int_compare(*target, ConstBinaryOp::Lt, end)
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
    let start = super::eval_resolved_const_int_expr_inner(start, env)?;
    let end = super::eval_resolved_const_int_expr_inner(end, env)?;
    Ok(if inclusive {
        super::eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && super::eval_binary_int_compare(*target, ConstBinaryOp::Le, end)
    } else {
        super::eval_binary_int_compare(start, ConstBinaryOp::Le, *target)
            && super::eval_binary_int_compare(*target, ConstBinaryOp::Lt, end)
    })
}

pub(super) fn bind_pattern_value(
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

pub(super) fn bind_function_pattern_value(
    binding: &ConstSwitchBinding,
    pattern_binding: &EarlyConstPatternBinding,
    env: &mut impl EarlyConstEnv,
) -> Result<(), ConstError> {
    env.bind_function_pattern_local(
        binding.span,
        pattern_binding,
        &binding.name,
        binding.local_id,
        binding.value.clone(),
    )
}

pub(super) fn bind_resolved_pattern_value(
    binding: &ConstSwitchBinding,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let local_id = binding
        .local_id
        .expect("resolved const switch pattern must have a local id");
    env.bind_resolved_pattern_local(binding.span, &binding.name, local_id, binding.value.clone())
}

pub(super) fn bind_resolved_function_pattern_value(
    binding: &ConstSwitchBinding,
    pattern_binding: &ResolvedConstPatternBinding,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let local_id = binding
        .local_id
        .expect("resolved const pattern binding must have a local id");
    env.bind_resolved_function_pattern_local(
        binding.span,
        pattern_binding,
        &binding.name,
        local_id,
        binding.value.clone(),
    )
}
