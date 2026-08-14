// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_struct_literal_flow(
    fields: &[nia_const_ir::EarlyConstFieldInit],
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    // Validate the complete field set before evaluating any initializer, so
    // an invalid literal cannot execute only a prefix of its expressions.
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

pub(super) fn eval_enum_struct_literal_flow(
    span: Span,
    variant: &EarlyConstExpr,
    fields: &[nia_const_ir::EarlyConstFieldInit],
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let Some(variant) = early_enum_expr_variant_id(variant, env) else {
        return Err(ConstError {
            span,
            message: "const enum literal requires a resolved enum variant".to_string(),
        });
    };
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
                "duplicate const enum field `{}`",
                env.symbol_name(field.name)
            ),
        });
    }
    let mut values = BTreeMap::new();
    for field in fields {
        values.insert(field.name, eval_value_or_return_flow!(&field.value, env));
    }
    Ok(ConstEvalFlow::Value(ConstValue::Enum {
        variant,
        payload: ConstEnumPayload::Named(values),
    }))
}

pub(super) fn eval_resolved_struct_literal_flow(
    span: Span,
    ty: InternedTyId,
    fields: &[ResolvedConstFieldInit],
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    // Keep resolved evaluation atomic with respect to structural validation,
    // matching the early evaluator above.
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
    Ok(ConstEvalFlow::Value(
        env.build_resolved_aggregate(span, ty, values)?,
    ))
}

pub(super) fn eval_resolved_enum_struct_literal_flow(
    span: Span,
    variant: &ResolvedConstExpr,
    fields: &[ResolvedConstFieldInit],
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let Some(variant) = resolved_enum_variant_id(variant, env) else {
        return Err(ConstError {
            span,
            message: "const enum literal requires a resolved enum variant".to_string(),
        });
    };
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
                "duplicate const enum field `{}`",
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
    Ok(ConstEvalFlow::Value(ConstValue::Enum {
        variant,
        payload: ConstEnumPayload::Named(values),
    }))
}

pub(super) fn early_enum_variant_id(
    name: &nia_const_ir::EarlyConstName,
    env: &impl ConstCommonEnv,
) -> Option<GlobalDefId> {
    let ConstNameResolution::Global(variant) = name.resolution()? else {
        return None;
    };
    env.is_enum_variant(variant).then_some(variant)
}

pub(super) fn early_enum_expr_variant_id(
    expr: &EarlyConstExpr,
    env: &impl ConstCommonEnv,
) -> Option<GlobalDefId> {
    match expr.kind() {
        EarlyConstExprKind::Ident(name) | EarlyConstExprKind::Qualified(name) => {
            early_enum_variant_id(name, env)
        }
        _ => None,
    }
}

pub(super) fn resolved_enum_variant_id(
    expr: &ResolvedConstExpr,
    env: &impl ConstCommonEnv,
) -> Option<GlobalDefId> {
    let ConstNameResolution::Global(variant) = expr.name_resolution()? else {
        return None;
    };
    env.is_enum_variant(variant).then_some(variant)
}
