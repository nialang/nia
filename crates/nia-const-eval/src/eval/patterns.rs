// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn early_pattern_matches(
    target: &ConstValue,
    pattern: &EarlyConstPattern,
    env: &mut impl EarlyConstEnv,
    bindings: &mut Vec<ConstMatchBinding>,
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
            bindings.push(ConstMatchBinding {
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
                message: "const optional match pattern requires an optional target".to_string(),
            }),
        },
        EarlyConstPattern::OptionalNull { span } => match target {
            ConstValue::Optional(None) => Ok(true),
            ConstValue::Optional(Some(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const null match pattern requires an optional target".to_string(),
            }),
        },
        EarlyConstPattern::ErrorOk { pattern, span } => match target {
            ConstValue::ErrorUnion(Ok(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error-ok match pattern requires an error union target".to_string(),
            }),
        },
        EarlyConstPattern::ErrorErr { pattern, span } => match target {
            ConstValue::ErrorUnion(Err(value)) => {
                early_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error match pattern requires an error union target".to_string(),
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
        } => match_range_matches(target, start, end, *inclusive, *span, env),
    }
}

pub(super) fn resolved_pattern_matches(
    target: &ConstValue,
    pattern: &nia_const_ir::ResolvedConstPattern,
    env: &mut impl ResolvedConstEnv,
    bindings: &mut Vec<ConstMatchBinding>,
) -> Result<bool, ConstError> {
    match pattern.kind() {
        ResolvedConstPatternKind::Wildcard { .. } => Ok(true),
        ResolvedConstPatternKind::Bind {
            name,
            local_id,
            span,
        } => {
            bindings.push(ConstMatchBinding {
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
                message: "const optional match pattern requires an optional target".to_string(),
            }),
        },
        ResolvedConstPatternKind::OptionalNull { span } => match target {
            ConstValue::Optional(None) => Ok(true),
            ConstValue::Optional(Some(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const null match pattern requires an optional target".to_string(),
            }),
        },
        ResolvedConstPatternKind::ErrorOk { pattern, span } => match target {
            ConstValue::ErrorUnion(Ok(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Err(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error-ok match pattern requires an error union target".to_string(),
            }),
        },
        ResolvedConstPatternKind::ErrorErr { pattern, span } => match target {
            ConstValue::ErrorUnion(Err(value)) => {
                resolved_pattern_matches(value, pattern, env, bindings)
            }
            ConstValue::ErrorUnion(Ok(_)) => Ok(false),
            _ => Err(ConstError {
                span: *span,
                message: "const error match pattern requires an error union target".to_string(),
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
        } => resolved_match_range_matches(target, start, end, *inclusive, *span, env),
    }
}

fn struct_pattern_matches<P>(
    target: &ConstValue,
    fields: &[nia_const_ir::ConstNamedPatternField<P>],
    bindings: &mut Vec<ConstMatchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstMatchBinding>,
    ) -> Result<bool, ConstError>,
) -> Result<bool, ConstError> {
    // Constructor identity and the complete field set are type-checking invariants. Evaluation
    // only performs the recursive value matched, using names because ConstValue stores struct fields
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
    bindings: &mut Vec<ConstMatchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstMatchBinding>,
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
        (
            ConstEnumPatternFields::Named {
                fields: patterns, ..
            },
            ConstEnumPayload::Named(values),
        ) => {
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
    bindings: &mut Vec<ConstMatchBinding>,
    span: Span,
    mut pattern_matches: impl FnMut(
        &ConstValue,
        &P,
        &mut Vec<ConstMatchBinding>,
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

fn match_range_matches(
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
            message: "const match range requires an integer target".to_string(),
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

fn resolved_match_range_matches(
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
            message: "const match range requires an integer target".to_string(),
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
    binding: &ConstMatchBinding,
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
    binding: &ConstMatchBinding,
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
    binding: &ConstMatchBinding,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let Some(local_id) = binding.local_id else {
        return Err(ConstError {
            span: binding.span,
            message: format!(
                "failed to resolve const pattern local `{}`",
                env.symbol_name(binding.name)
            ),
        });
    };
    env.bind_resolved_pattern_local(binding.span, &binding.name, local_id, binding.value.clone())
}

pub(super) fn bind_resolved_function_pattern_value(
    binding: &ConstMatchBinding,
    pattern_binding: &ResolvedConstPatternBinding,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let Some(local_id) = binding.local_id else {
        return Err(ConstError {
            span: binding.span,
            message: format!(
                "failed to resolve const pattern local `{}`",
                env.symbol_name(binding.name)
            ),
        });
    };
    env.bind_resolved_function_pattern_local(
        binding.span,
        pattern_binding,
        &binding.name,
        local_id,
        binding.value.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConstCommonEnv, ResolvedConstEnv};
    use nia_const_ir::{ResolvedConstExpr, ResolvedConstPattern, ResolvedConstPatternBinding};
    use nia_ids::LayoutBuiltin;
    use nia_symbol::{SymbolId, stable_hash};

    struct TestResolvedEnv;

    impl ConstCommonEnv for TestResolvedEnv {}

    impl ResolvedConstEnv for TestResolvedEnv {
        fn resolve_resolved_name(
            &mut self,
            span: Span,
            _resolution: nia_const_ir::ConstNameResolution,
        ) -> Result<ConstValue, ConstError> {
            Err(ConstError {
                span,
                message: "unexpected resolved name lookup".to_string(),
            })
        }

        fn resolve_resolved_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg: &nia_const_ir::ResolvedConstTypeArg,
        ) -> Result<ConstValue, ConstError> {
            Err(ConstError {
                span,
                message: "unexpected layout builtin lookup".to_string(),
            })
        }
    }

    fn payload_name() -> SymbolId {
        SymbolId::from_stable_hash(stable_hash("payload"))
    }

    #[test]
    fn resolved_pattern_binding_without_local_id_is_an_error() {
        let binding = ConstMatchBinding {
            span: Span::new(4, 11),
            name: payload_name(),
            local_id: None,
            value: ConstValue::Int(nia_ty::IntConst::signed(1)),
        };
        let mut env = TestResolvedEnv;
        let expected_name = env.symbol_name(binding.name);
        let error = bind_resolved_pattern_value(&binding, &mut env)
            .expect_err("missing resolved pattern locals must be diagnosed");
        assert_eq!(
            error.message,
            format!("failed to resolve const pattern local `{expected_name}`")
        );
    }

    #[test]
    fn resolved_function_pattern_binding_without_local_id_is_an_error() {
        let binding = ConstMatchBinding {
            span: Span::new(4, 11),
            name: payload_name(),
            local_id: None,
            value: ConstValue::Int(nia_ty::IntConst::signed(1)),
        };
        let pattern_binding = ResolvedConstPatternBinding::new(
            binding.span,
            ResolvedConstPattern::wildcard(binding.span),
            None,
            false,
            ResolvedConstExpr::from_parts(
                binding.span,
                nia_const_ir::ResolvedConstExprKind::Integer("1".to_string()),
            ),
        );
        let mut env = TestResolvedEnv;
        let expected_name = env.symbol_name(binding.name);
        let error = bind_resolved_function_pattern_value(&binding, &pattern_binding, &mut env)
            .expect_err("missing resolved function pattern locals must be diagnosed");
        assert_eq!(
            error.message,
            format!("failed to resolve const pattern local `{expected_name}`")
        );
    }
}
