// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl BodyChecker<'_> {
    pub(super) fn check_pattern_matrix_exhaustive(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        matrix: &[Vec<AnalysisPattern<PatternConstructor>>],
    ) {
        match missing_witness(matrix, target_ty, |ty| self.analysis_domain(*ty)) {
            Ok(None) => {}
            Ok(Some(witness)) => {
                let witness = self.format_analysis_witness(&witness, target_ty);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("non-exhaustive switch, missing pattern: `{witness}`"),
                ));
            }
            Err(error) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("cannot analyze switch exhaustiveness: {error}"),
            )),
        }
    }

    pub(super) fn analysis_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        target_ty: InternedTyId,
    ) -> AnalysisPattern<PatternConstructor> {
        let normalized = self.normalization.normalize(target_ty);
        match &pattern.kind {
            nia_ast::PatternKind::Wildcard | nia_ast::PatternKind::Bind { .. } => {
                AnalysisPattern::Wildcard
            }
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let expected_readonly = matches!(pattern.kind, nia_ast::PatternKind::Pointer(_));
                let Some(TyKind::Pointer { is_readonly, elem }) =
                    self.interner.get(normalized).cloned()
                else {
                    return AnalysisPattern::Opaque;
                };
                if is_readonly != expected_readonly {
                    return AnalysisPattern::Opaque;
                }
                AnalysisPattern::Constructor {
                    id: PatternConstructor::Pointer { is_readonly },
                    fields: vec![self.analysis_pattern(inner, elem)],
                }
            }
            nia_ast::PatternKind::OptionalSome(inner) => {
                let Some(TyKind::Optional { elem }) = self.interner.get(normalized).cloned() else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: PatternConstructor::OptionalSome,
                    fields: vec![self.analysis_pattern(inner, elem)],
                }
            }
            nia_ast::PatternKind::OptionalNull => AnalysisPattern::Constructor {
                id: PatternConstructor::OptionalNull,
                fields: Vec::new(),
            },
            nia_ast::PatternKind::ErrorOk(inner) => {
                let Some(TyKind::ErrorUnion { value, .. }) = self.interner.get(normalized).cloned()
                else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: PatternConstructor::ErrorOk,
                    fields: vec![self.analysis_pattern(inner, value)],
                }
            }
            nia_ast::PatternKind::ErrorErr(inner) => {
                let Some(TyKind::ErrorUnion { error, .. }) = self.interner.get(normalized).cloned()
                else {
                    return AnalysisPattern::Opaque;
                };
                AnalysisPattern::Constructor {
                    id: PatternConstructor::ErrorErr,
                    fields: vec![self.analysis_pattern(inner, error)],
                }
            }
            nia_ast::PatternKind::Tuple(patterns) => {
                let Some(TyKind::Tuple(elems)) = self.interner.get(normalized).cloned() else {
                    return AnalysisPattern::Opaque;
                };
                if patterns.len() != elems.len() {
                    return AnalysisPattern::Opaque;
                }
                AnalysisPattern::Constructor {
                    id: PatternConstructor::Tuple,
                    fields: patterns
                        .iter()
                        .zip(elems)
                        .map(|(pattern, ty)| self.analysis_pattern(pattern, ty))
                        .collect(),
                }
            }
            nia_ast::PatternKind::Nominal {
                constructor,
                fields,
            } => self.analysis_nominal_pattern(constructor, fields, target_ty),
            nia_ast::PatternKind::Expr(expr) => {
                if let Some((enum_id, variant_def)) = self.enum_variant_info(expr)
                    && self.enum_global_def_id(target_ty) == Some(enum_id)
                    && let Some((_, variant)) = self.resolved_enum_variant(GlobalDefId {
                        module_id: enum_id.module_id,
                        def_id: variant_def,
                    })
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
                        id: PatternConstructor::EnumVariant(GlobalDefId {
                            module_id: enum_id.module_id,
                            def_id: variant_def,
                        }),
                        fields,
                    };
                }
                let Some(value) = self.node_pattern_values.get(&expr.node_key).copied() else {
                    return AnalysisPattern::Opaque;
                };
                match self.analysis_domain(target_ty) {
                    AnalysisDomain::Scalar { min, max, .. } if min <= value && value <= max => {
                        AnalysisPattern::ScalarRange {
                            start: value,
                            end: value,
                        }
                    }
                    _ => AnalysisPattern::Opaque,
                }
            }
            nia_ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                if !matches!(
                    self.interner.get(normalized),
                    Some(TyKind::Primitive(primitive)) if primitive.is_integer()
                ) {
                    return AnalysisPattern::Opaque;
                }
                let Some(start) = self.node_pattern_values.get(&start.node_key).copied() else {
                    return AnalysisPattern::Opaque;
                };
                let Some(mut end) = self.node_pattern_values.get(&end.node_key).copied() else {
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
                match self.analysis_domain(target_ty) {
                    AnalysisDomain::Scalar { min, max, .. } if min <= start && end <= max => {
                        AnalysisPattern::ScalarRange { start, end }
                    }
                    _ => AnalysisPattern::Opaque,
                }
            }
        }
    }

    fn analysis_nominal_pattern(
        &mut self,
        constructor: &Expr,
        fields: &nia_ast::NominalPatternFields,
        target_ty: InternedTyId,
    ) -> AnalysisPattern<PatternConstructor> {
        if let Some((enum_id, variant_def)) = self.enum_variant_info(constructor) {
            if self.enum_global_def_id(target_ty) != Some(enum_id) {
                return AnalysisPattern::Opaque;
            }
            let variant_id = GlobalDefId {
                module_id: enum_id.module_id,
                def_id: variant_def,
            };
            let Some((_, variant)) = self.resolved_enum_variant(variant_id) else {
                return AnalysisPattern::Opaque;
            };
            let normalized_fields = match (&variant.payload, fields) {
                (
                    nia_item_signatures::EnumVariantPayloadSignature::Tuple(expected),
                    nia_ast::NominalPatternFields::Tuple(actual),
                ) if expected.len() == actual.len() => actual
                    .iter()
                    .zip(expected)
                    .map(|(pattern, ty)| self.analysis_pattern(pattern, *ty))
                    .collect(),
                (
                    nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
                    nia_ast::NominalPatternFields::Named {
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
                        || expected.iter().all(|field| {
                            actual.iter().any(|actual| actual.name == field.name)
                        })) =>
                {
                    expected
                        .iter()
                        .map(|field| {
                            actual
                                .iter()
                                .find(|actual| actual.name == field.name)
                                .map_or(AnalysisPattern::Wildcard, |actual| {
                                    self.analysis_pattern(&actual.pattern, field.ty)
                                })
                        })
                        .collect()
                }
                _ => return AnalysisPattern::Opaque,
            };
            return AnalysisPattern::Constructor {
                id: PatternConstructor::EnumVariant(variant_id),
                fields: normalized_fields,
            };
        }

        let Some((constructor_def, _, _)) = self.type_prefix_instance(constructor) else {
            return AnalysisPattern::Opaque;
        };
        let normalized = self.normalization.normalize(target_ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(normalized).cloned() else {
            return AnalysisPattern::Opaque;
        };
        if def_id != constructor_def {
            return AnalysisPattern::Opaque;
        }
        let nia_ast::NominalPatternFields::Named {
            fields: actual,
            rest,
        } = fields
        else {
            return AnalysisPattern::Opaque;
        };
        let Some(signature) = self.resolved_struct_signature(def_id) else {
            return AnalysisPattern::Opaque;
        };
        if !actual.iter().all(|actual| {
            signature
                .signature
                .fields
                .iter()
                .any(|field| actual.name == field.name)
        }) || actual.iter().any(|field| {
            actual
                .iter()
                .filter(|other| other.name == field.name)
                .count()
                != 1
        }) || (rest.is_none()
            && signature
                .signature
                .fields
                .iter()
                .any(|field| !actual.iter().any(|actual| actual.name == field.name)))
        {
            return AnalysisPattern::Opaque;
        }
        let mut normalized_fields = Vec::with_capacity(actual.len());
        for expected in signature.signature.fields {
            let Some(ty) = self.field_ty_for_aggregate_ty(target_ty, &expected.name) else {
                return AnalysisPattern::Opaque;
            };
            normalized_fields.push(
                actual
                    .iter()
                    .find(|actual| actual.name == expected.name)
                    .map_or(AnalysisPattern::Wildcard, |actual| {
                        self.analysis_pattern(&actual.pattern, ty)
                    }),
            );
        }
        AnalysisPattern::Constructor {
            id: PatternConstructor::Struct(def_id),
            fields: normalized_fields,
        }
    }

    pub(super) fn analysis_domain(
        &mut self,
        target_ty: InternedTyId,
    ) -> AnalysisDomain<InternedTyId, PatternConstructor> {
        let normalized = self.normalization.normalize(target_ty);
        match self.interner.get(normalized).cloned() {
            Some(TyKind::Tuple(fields)) => AnalysisDomain::Finite(vec![AnalysisConstructor {
                id: PatternConstructor::Tuple,
                fields,
            }]),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                AnalysisDomain::Finite(vec![AnalysisConstructor {
                    id: PatternConstructor::Pointer { is_readonly },
                    fields: vec![elem],
                }])
            }
            Some(TyKind::Optional { elem }) => AnalysisDomain::Finite(vec![
                AnalysisConstructor {
                    id: PatternConstructor::OptionalNull,
                    fields: Vec::new(),
                },
                AnalysisConstructor {
                    id: PatternConstructor::OptionalSome,
                    fields: vec![elem],
                },
            ]),
            Some(TyKind::ErrorUnion { error, value }) => AnalysisDomain::Finite(vec![
                AnalysisConstructor {
                    id: PatternConstructor::ErrorOk,
                    fields: vec![value],
                },
                AnalysisConstructor {
                    id: PatternConstructor::ErrorErr,
                    fields: vec![error],
                },
            ]),
            Some(TyKind::Primitive(PrimitiveTy::Bool)) => AnalysisDomain::Scalar {
                min: 0,
                max: 1,
                complete: true,
            },
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                let bits = primitive
                    .integer_bits(self.target.pointer_width)
                    .unwrap_or(i128::BITS);
                let signed = primitive.is_signed_integer();
                let (min, max, complete) = if signed && bits >= i128::BITS {
                    (i128::MIN, i128::MAX, true)
                } else if signed {
                    let magnitude = 1i128 << (bits - 1);
                    (-magnitude, magnitude - 1, true)
                } else if bits >= i128::BITS {
                    (0, i128::MAX, false)
                } else {
                    (0, (1i128 << bits) - 1, true)
                };
                AnalysisDomain::Scalar { min, max, complete }
            }
            Some(TyKind::Primitive(PrimitiveTy::Never)) => AnalysisDomain::Finite(Vec::new()),
            Some(TyKind::Nominal { def_id, .. }) => {
                if let Some(resolved) = self.resolved_enum_signature(def_id) {
                    let constructors = resolved
                        .signature
                        .variants
                        .into_iter()
                        .map(|variant| AnalysisConstructor {
                            id: PatternConstructor::EnumVariant(GlobalDefId {
                                module_id: def_id.module_id,
                                def_id: variant.def_id,
                            }),
                            fields: match variant.payload {
                                nia_item_signatures::EnumVariantPayloadSignature::Unit => {
                                    Vec::new()
                                }
                                nia_item_signatures::EnumVariantPayloadSignature::Tuple(fields) => {
                                    fields
                                }
                                nia_item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                                    fields.into_iter().map(|field| field.ty).collect()
                                }
                            },
                        })
                        .collect();
                    if resolved.signature.is_open {
                        AnalysisDomain::Open(constructors)
                    } else {
                        AnalysisDomain::Finite(constructors)
                    }
                } else if let Some(resolved) = self.resolved_struct_signature(def_id) {
                    let mut fields = Vec::with_capacity(resolved.signature.fields.len());
                    for field in resolved.signature.fields {
                        let Some(ty) = self.field_ty_for_aggregate_ty(target_ty, &field.name)
                        else {
                            return AnalysisDomain::Opaque;
                        };
                        fields.push(ty);
                    }
                    AnalysisDomain::Finite(vec![AnalysisConstructor {
                        id: PatternConstructor::Struct(def_id),
                        fields,
                    }])
                } else {
                    AnalysisDomain::Opaque
                }
            }
            _ => AnalysisDomain::Opaque,
        }
    }

    fn format_analysis_witness(
        &mut self,
        pattern: &AnalysisPattern<PatternConstructor>,
        target_ty: InternedTyId,
    ) -> String {
        match pattern {
            AnalysisPattern::Wildcard | AnalysisPattern::Opaque => "_".to_string(),
            AnalysisPattern::ScalarRange { start, .. } if self.is_bool(target_ty) => {
                if *start == 0 { "false" } else { "true" }.to_string()
            }
            AnalysisPattern::ScalarRange { start, .. } => start.to_string(),
            AnalysisPattern::Constructor { id, fields } => {
                let field_types = match self.analysis_domain(target_ty) {
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
                    .map(|(field, ty)| self.format_analysis_witness(field, ty))
                    .collect();
                match id {
                    PatternConstructor::Tuple => format!("({})", formatted.join(", ")),
                    PatternConstructor::Pointer { is_readonly: true } => {
                        format!("&{}", formatted.join(""))
                    }
                    PatternConstructor::Pointer { is_readonly: false } => {
                        format!("&mut {}", formatted.join(""))
                    }
                    PatternConstructor::OptionalSome => format!("?{}", formatted.join("")),
                    PatternConstructor::OptionalNull => "null".to_string(),
                    PatternConstructor::ErrorOk => format!("!{}", formatted.join("")),
                    PatternConstructor::ErrorErr => format!("{}!", formatted.join("")),
                    PatternConstructor::Struct(def_id) => {
                        let fields = self
                            .resolved_struct_signature(*def_id)
                            .map(|signature| {
                                signature
                                    .signature
                                    .fields
                                    .into_iter()
                                    .zip(&formatted)
                                    .map(|(field, value)| {
                                        format!("{}: {value}", self.symbol_name(field.name))
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_else(|| formatted.join(", "));
                        format!("{} {{ {fields} }}", self.ty_name(target_ty))
                    }
                    PatternConstructor::EnumVariant(variant_id) => {
                        let variant = self.resolved_enum_variant(*variant_id);
                        let name = variant
                            .as_ref()
                            .map(|(_, variant)| self.symbol_name(variant.name))
                            .unwrap_or_else(|| "<variant>".to_string());
                        if formatted.is_empty() {
                            name
                        } else if let Some((_, variant)) = variant
                            && let nia_item_signatures::EnumVariantPayloadSignature::Named(fields) =
                                variant.payload
                        {
                            let fields = fields
                                .into_iter()
                                .zip(formatted)
                                .map(|(field, value)| {
                                    format!("{}: {value}", self.symbol_name(field.name))
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("{name} {{ {fields} }}")
                        } else {
                            format!("{name}({})", formatted.join(", "))
                        }
                    }
                }
            }
        }
    }
}
