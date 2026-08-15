// SPDX-License-Identifier: GPL-3.0-or-later
//! Const-switch adapter for the shared constructor-matrix analysis.
//!
//! Resolved const patterns use the same constructor identities and declaration
//! field order as runtime checking, but resolve scalar values through the const
//! evaluator and preserve const-specific module/type substitutions. This module
//! only performs coverage analysis; const execution remains path-driven.
use super::*;

impl Analyzer<'_> {
    pub(in crate::analyzer) fn check_resolved_const_switch_coverage(
        &mut self,
        switch: &ResolvedConstSwitch,
        target_ty: InternedTyId,
    ) {
        let mut matrix = Vec::new();
        for arm in switch.arms() {
            for pattern in arm.patterns() {
                let normalized = self.const_analysis_pattern(pattern, target_ty);
                match useful_witness(
                    &matrix,
                    std::slice::from_ref(&normalized),
                    &[target_ty],
                    |ty| self.const_analysis_domain(*ty),
                ) {
                    Ok(Some(_)) => matrix.push(vec![normalized]),
                    Ok(None) => self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        pattern.span(),
                        "const switch pattern is unreachable because previous patterns cover all of its values",
                    )),
                    Err(error) => self.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        pattern.span(),
                        format!("cannot analyze const switch pattern coverage: {error}"),
                    )),
                }
            }
        }
        match missing_witness(&matrix, target_ty, |ty| self.const_analysis_domain(*ty)) {
            Ok(None) => {}
            Ok(Some(witness)) => {
                let witness = self.format_const_analysis_witness(&witness, target_ty);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    switch.target().span(),
                    format!("non-exhaustive const switch, missing pattern: `{witness}`"),
                ));
            }
            Err(error) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                switch.target().span(),
                format!("cannot analyze const switch exhaustiveness: {error}"),
            )),
        }
    }

    fn const_analysis_pattern(
        &mut self,
        pattern: &ResolvedConstPattern,
        target_ty: InternedTyId,
    ) -> AnalysisPattern<ConstPatternConstructor> {
        match pattern.kind() {
            ResolvedConstPatternKind::Wildcard { .. } | ResolvedConstPatternKind::Bind { .. } => {
                AnalysisPattern::Wildcard
            }
            ResolvedConstPatternKind::Pointer { pattern, .. } => {
                let Some(TyKind::Pointer { is_readonly, elem }) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                if !is_readonly {
                    return AnalysisPattern::Opaque;
                }
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::Pointer { is_readonly },
                    fields: vec![self.const_analysis_pattern(pattern, elem)],
                }
            }
            ResolvedConstPatternKind::MutPointer { pattern, .. } => {
                let Some(TyKind::Pointer { is_readonly, elem }) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                if is_readonly {
                    return AnalysisPattern::Opaque;
                }
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::Pointer { is_readonly },
                    fields: vec![self.const_analysis_pattern(pattern, elem)],
                }
            }
            ResolvedConstPatternKind::OptionalSome { pattern, .. } => {
                let Some(TyKind::Optional { elem }) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::OptionalSome,
                    fields: vec![self.const_analysis_pattern(pattern, elem)],
                }
            }
            ResolvedConstPatternKind::OptionalNull { .. } => AnalysisPattern::Constructor {
                id: ConstPatternConstructor::OptionalNull,
                fields: Vec::new(),
            },
            ResolvedConstPatternKind::ErrorOk { pattern, .. } => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::ErrorOk,
                    fields: vec![self.const_analysis_pattern(pattern, value)],
                }
            }
            ResolvedConstPatternKind::ErrorErr { pattern, .. } => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::ErrorErr,
                    fields: vec![self.const_analysis_pattern(pattern, error)],
                }
            }
            ResolvedConstPatternKind::Tuple { patterns, .. } => {
                let Some(TyKind::Tuple(elems)) = self.ty_kind(target_ty) else {
                    return AnalysisPattern::Opaque;
                };
                if patterns.len() != elems.len() {
                    return AnalysisPattern::Opaque;
                }
                AnalysisPattern::Constructor {
                    id: ConstPatternConstructor::Tuple,
                    fields: patterns
                        .iter()
                        .zip(elems)
                        .map(|(pattern, ty)| self.const_analysis_pattern(pattern, ty))
                        .collect(),
                }
            }
            ResolvedConstPatternKind::EnumVariant {
                variant, fields, ..
            } => self.const_analysis_enum_pattern(variant, fields, target_ty),
            ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                ..
            } => self.const_analysis_struct_pattern(*def_id, fields, *rest, target_ty),
            ResolvedConstPatternKind::Expr(expr) => {
                if let Some((enum_id, variant)) = self.resolved_const_enum_variant(expr)
                    && self.expected_nominal_parts(target_ty).map(|parts| parts.0) == Some(enum_id)
                {
                    let fields = match variant.payload {
                        nia_item_signatures::EnumVariantPayloadSignature::Unit => Vec::new(),
                        nia_item_signatures::EnumVariantPayloadSignature::Tuple(fields) => {
                            vec![AnalysisPattern::Wildcard; fields.len()]
                        }
                        nia_item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                            vec![AnalysisPattern::Wildcard; fields.len()]
                        }
                    };
                    return AnalysisPattern::Constructor {
                        id: ConstPatternConstructor::EnumVariant(GlobalDefId {
                            module_id: enum_id.module_id,
                            def_id: variant.def_id,
                        }),
                        fields,
                    };
                }
                let Some(value) = self.const_analysis_scalar_value(expr) else {
                    return AnalysisPattern::Opaque;
                };
                match self.const_analysis_domain(target_ty) {
                    AnalysisDomain::Scalar { min, max, .. } if min <= value && value <= max => {
                        AnalysisPattern::ScalarRange {
                            start: value,
                            end: value,
                        }
                    }
                    _ => AnalysisPattern::Opaque,
                }
            }
            ResolvedConstPatternKind::Range {
                start,
                end,
                inclusive,
                ..
            } => {
                if !self.is_integer_runtime_type(target_ty) {
                    return AnalysisPattern::Opaque;
                }
                let Some(start) = self.const_analysis_scalar_value(start) else {
                    return AnalysisPattern::Opaque;
                };
                let Some(mut end) = self.const_analysis_scalar_value(end) else {
                    return AnalysisPattern::Opaque;
                };
                if !inclusive {
                    let Some(exclusive_end) = end.checked_sub(1) else {
                        return AnalysisPattern::Opaque;
                    };
                    end = exclusive_end;
                }
                if start > end {
                    return AnalysisPattern::Opaque;
                }
                match self.const_analysis_domain(target_ty) {
                    AnalysisDomain::Scalar { min, max, .. } if min <= start && end <= max => {
                        AnalysisPattern::ScalarRange { start, end }
                    }
                    _ => AnalysisPattern::Opaque,
                }
            }
        }
    }

    fn const_analysis_enum_pattern(
        &mut self,
        variant_expr: &ResolvedConstExpr,
        fields: &ConstEnumPatternFields<ResolvedConstPattern>,
        target_ty: InternedTyId,
    ) -> AnalysisPattern<ConstPatternConstructor> {
        let Some((enum_id, variant)) = self.resolved_const_enum_variant(variant_expr) else {
            return AnalysisPattern::Opaque;
        };
        if self.expected_nominal_parts(target_ty).map(|parts| parts.0) != Some(enum_id) {
            return AnalysisPattern::Opaque;
        }
        let current_module = self.current_execution_module_id();
        let normalized = match (&variant.payload, fields) {
            (
                nia_item_signatures::EnumVariantPayloadSignature::Tuple(expected),
                ConstEnumPatternFields::Tuple(actual),
            ) if expected.len() == actual.len() => {
                let mut normalized = Vec::with_capacity(actual.len());
                for (pattern, ty) in actual.iter().zip(expected) {
                    let Some(ty) = self.type_for_module_or_none(*ty, current_module) else {
                        return AnalysisPattern::Opaque;
                    };
                    normalized.push(self.const_analysis_pattern(pattern, ty));
                }
                normalized
            }
            (
                nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
                ConstEnumPatternFields::Named {
                    fields: actual,
                    rest,
                },
            ) if actual
                .iter()
                .all(|actual| expected.iter().any(|field| actual.name == field.name))
                && actual.iter().all(|field| {
                    actual
                        .iter()
                        .filter(|other| other.name == field.name)
                        .count()
                        == 1
                })
                && (rest.is_some()
                    || expected
                        .iter()
                        .all(|field| actual.iter().any(|actual| actual.name == field.name))) =>
            {
                let mut normalized = Vec::with_capacity(expected.len());
                for field in expected {
                    let Some(ty) = self.type_for_module_or_none(field.ty, current_module) else {
                        return AnalysisPattern::Opaque;
                    };
                    normalized.push(
                        actual
                            .iter()
                            .find(|actual| actual.name == field.name)
                            .map_or(AnalysisPattern::Wildcard, |actual| {
                                self.const_analysis_pattern(&actual.pattern, ty)
                            }),
                    );
                }
                normalized
            }
            _ => return AnalysisPattern::Opaque,
        };
        AnalysisPattern::Constructor {
            id: ConstPatternConstructor::EnumVariant(GlobalDefId {
                module_id: enum_id.module_id,
                def_id: variant.def_id,
            }),
            fields: normalized,
        }
    }

    fn const_analysis_struct_pattern(
        &mut self,
        def_id: GlobalDefId,
        fields: &[ConstNamedPatternField<ResolvedConstPattern>],
        rest: Option<Span>,
        target_ty: InternedTyId,
    ) -> AnalysisPattern<ConstPatternConstructor> {
        if self.expected_nominal_parts(target_ty).map(|parts| parts.0) != Some(def_id) {
            return AnalysisPattern::Opaque;
        }
        let Some(signature) = self.struct_signature_for(def_id) else {
            return AnalysisPattern::Opaque;
        };
        if !fields.iter().all(|actual| {
            signature
                .fields
                .iter()
                .any(|field| actual.name == field.name)
        }) || fields.iter().any(|field| {
            fields
                .iter()
                .filter(|other| other.name == field.name)
                .count()
                != 1
        }) || (rest.is_none()
            && signature
                .fields
                .iter()
                .any(|field| !fields.iter().any(|actual| actual.name == field.name)))
        {
            return AnalysisPattern::Opaque;
        }
        let mut normalized = Vec::with_capacity(signature.fields.len());
        for expected in signature.fields {
            let Some(ty) = self.const_nominal_aggregate_field_type(target_ty, &expected.name)
            else {
                return AnalysisPattern::Opaque;
            };
            normalized.push(
                fields
                    .iter()
                    .find(|actual| actual.name == expected.name)
                    .map_or(AnalysisPattern::Wildcard, |actual| {
                        self.const_analysis_pattern(&actual.pattern, ty)
                    }),
            );
        }
        AnalysisPattern::Constructor {
            id: ConstPatternConstructor::Struct(def_id),
            fields: normalized,
        }
    }

    fn const_analysis_scalar_value(&mut self, expr: &ResolvedConstExpr) -> Option<i128> {
        match expr.kind() {
            ResolvedConstExprKind::Bool(value) => Some(i128::from(*value)),
            _ => match nia_const_eval::eval_resolved_const_expr(expr, self).ok()? {
                ConstValue::Int(value) => value.as_i128(),
                ConstValue::Bool(value) => Some(i128::from(value)),
                _ => None,
            },
        }
    }

    fn const_analysis_domain(
        &mut self,
        target_ty: InternedTyId,
    ) -> AnalysisDomain<InternedTyId, ConstPatternConstructor> {
        match self.ty_kind(target_ty) {
            Some(TyKind::Tuple(fields)) => AnalysisDomain::Finite(vec![AnalysisConstructor {
                id: ConstPatternConstructor::Tuple,
                fields,
            }]),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                AnalysisDomain::Finite(vec![AnalysisConstructor {
                    id: ConstPatternConstructor::Pointer { is_readonly },
                    fields: vec![elem],
                }])
            }
            Some(TyKind::Optional { elem }) => AnalysisDomain::Finite(vec![
                AnalysisConstructor {
                    id: ConstPatternConstructor::OptionalNull,
                    fields: Vec::new(),
                },
                AnalysisConstructor {
                    id: ConstPatternConstructor::OptionalSome,
                    fields: vec![elem],
                },
            ]),
            Some(TyKind::ErrorUnion { error, value }) => AnalysisDomain::Finite(vec![
                AnalysisConstructor {
                    id: ConstPatternConstructor::ErrorOk,
                    fields: vec![value],
                },
                AnalysisConstructor {
                    id: ConstPatternConstructor::ErrorErr,
                    fields: vec![error],
                },
            ]),
            Some(TyKind::Primitive(PrimitiveTy::Bool)) => AnalysisDomain::Scalar {
                min: 0,
                max: 1,
                complete: true,
            },
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                let Some((min, max)) =
                    primitive_integer_range_for_target(primitive, self.input.target.pointer_width)
                else {
                    return AnalysisDomain::Opaque;
                };
                let complete = primitive != PrimitiveTy::U128
                    && !(primitive == PrimitiveTy::Usize
                        && self.input.target.pointer_width >= i128::BITS);
                AnalysisDomain::Scalar { min, max, complete }
            }
            Some(TyKind::Primitive(PrimitiveTy::Never)) => AnalysisDomain::Finite(Vec::new()),
            Some(TyKind::Nominal { def_id, .. }) => {
                if let Some(signature) = self
                    .signatures_for_module(def_id.module_id)
                    .and_then(|signatures| signatures.as_ref().enums.get(&def_id.def_id).cloned())
                {
                    let current_module = self.current_execution_module_id();
                    let mut constructors = Vec::with_capacity(signature.variants.len());
                    for variant in signature.variants {
                        let source_fields = match variant.payload {
                            nia_item_signatures::EnumVariantPayloadSignature::Unit => Vec::new(),
                            nia_item_signatures::EnumVariantPayloadSignature::Tuple(fields) => {
                                fields
                            }
                            nia_item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                                fields.into_iter().map(|field| field.ty).collect()
                            }
                        };
                        let mut fields = Vec::with_capacity(source_fields.len());
                        for field in source_fields {
                            let Some(field) = self.type_for_module_or_none(field, current_module)
                            else {
                                return AnalysisDomain::Opaque;
                            };
                            fields.push(field);
                        }
                        constructors.push(AnalysisConstructor {
                            id: ConstPatternConstructor::EnumVariant(GlobalDefId {
                                module_id: def_id.module_id,
                                def_id: variant.def_id,
                            }),
                            fields,
                        });
                    }
                    if signature.is_open {
                        AnalysisDomain::Open(constructors)
                    } else {
                        AnalysisDomain::Finite(constructors)
                    }
                } else if let Some(signature) = self.struct_signature_for(def_id) {
                    let mut fields = Vec::with_capacity(signature.fields.len());
                    for field in signature.fields {
                        let Some(ty) =
                            self.const_nominal_aggregate_field_type(target_ty, &field.name)
                        else {
                            return AnalysisDomain::Opaque;
                        };
                        fields.push(ty);
                    }
                    AnalysisDomain::Finite(vec![AnalysisConstructor {
                        id: ConstPatternConstructor::Struct(def_id),
                        fields,
                    }])
                } else {
                    AnalysisDomain::Opaque
                }
            }
            _ => AnalysisDomain::Opaque,
        }
    }

    fn format_const_analysis_witness(
        &mut self,
        pattern: &AnalysisPattern<ConstPatternConstructor>,
        target_ty: InternedTyId,
    ) -> String {
        match pattern {
            AnalysisPattern::Wildcard | AnalysisPattern::Opaque => "_".to_string(),
            AnalysisPattern::ScalarRange { start, .. }
                if matches!(
                    self.ty_kind(target_ty),
                    Some(TyKind::Primitive(PrimitiveTy::Bool))
                ) =>
            {
                if *start == 0 { "false" } else { "true" }.to_string()
            }
            AnalysisPattern::ScalarRange { start, .. } => start.to_string(),
            AnalysisPattern::Constructor { id, fields } => {
                let field_types = match self.const_analysis_domain(target_ty) {
                    AnalysisDomain::Finite(constructors) | AnalysisDomain::Open(constructors) => {
                        constructors
                            .into_iter()
                            .find(|constructor| constructor.id == *id)
                            .map(|constructor| constructor.fields)
                            .unwrap_or_default()
                    }
                    AnalysisDomain::Scalar { .. } | AnalysisDomain::Opaque => Vec::new(),
                };
                let formatted: Vec<String> = fields
                    .iter()
                    .zip(field_types)
                    .map(|(field, ty)| self.format_const_analysis_witness(field, ty))
                    .collect();
                match id {
                    ConstPatternConstructor::Tuple => format!("({})", formatted.join(", ")),
                    ConstPatternConstructor::Pointer { is_readonly: true } => {
                        format!("&{}", formatted.join(""))
                    }
                    ConstPatternConstructor::Pointer { is_readonly: false } => {
                        format!("&mut {}", formatted.join(""))
                    }
                    ConstPatternConstructor::OptionalSome => format!("?{}", formatted.join("")),
                    ConstPatternConstructor::OptionalNull => "null".to_string(),
                    ConstPatternConstructor::ErrorOk => format!("!{}", formatted.join("")),
                    ConstPatternConstructor::ErrorErr => format!("{}!", formatted.join("")),
                    ConstPatternConstructor::Struct(_) => {
                        format!("struct {{ {} }}", formatted.join(", "))
                    }
                    ConstPatternConstructor::EnumVariant(variant_id) => {
                        let name = self
                            .signatures_for_module(variant_id.module_id)
                            .and_then(|signatures| {
                                signatures.as_ref().enums.values().find_map(|signature| {
                                    signature
                                        .variants
                                        .iter()
                                        .find(|variant| variant.def_id == variant_id.def_id)
                                        .map(|variant| self.symbol_name(variant.name))
                                })
                            })
                            .unwrap_or_else(|| "<variant>".to_string());
                        if formatted.is_empty() {
                            name
                        } else {
                            format!("{name}({})", formatted.join(", "))
                        }
                    }
                }
            }
        }
    }
}
