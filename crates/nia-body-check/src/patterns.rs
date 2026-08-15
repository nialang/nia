// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

use nia_pattern_analysis::{Constructor as AnalysisConstructor, Domain as AnalysisDomain};
use nia_pattern_analysis::{Pattern as AnalysisPattern, missing_witness, useful_witness};

mod analysis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternConstructor {
    Tuple,
    Pointer { is_readonly: bool },
    OptionalSome,
    OptionalNull,
    ErrorOk,
    ErrorErr,
    Struct(GlobalDefId),
    EnumVariant(GlobalDefId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatchInterval {
    start: i128,
    end: i128,
    span: Span,
}

struct RangePatternCheck<'a> {
    span: Span,
    start: &'a Expr,
    end: &'a Expr,
    inclusive: bool,
}

#[derive(Debug, Clone, Default)]
struct PatternCoverage {
    catch_all: Option<Span>,
    optional_null: Option<Span>,
    optional_some: Option<Box<PatternCoverage>>,
    error_ok: Option<Box<PatternCoverage>>,
    error_err: Option<Box<PatternCoverage>>,
    intervals: Vec<MatchInterval>,
    enum_variants: HashMap<DefId, Span>,
    single_field_enum_payloads: HashMap<DefId, (InternedTyId, Box<PatternCoverage>)>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_match_expr(
        &mut self,
        matched: &nia_ast::MatchExpr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&matched.target);
        let mut matrix = Vec::new();
        let mut result_ty = expected;

        for arm in &matched.arms {
            if arm.patterns.len() > 1 && arm.patterns.iter().any(nia_ast::Pattern::contains_binding)
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    arm.span,
                    "match arms with multiple alternative patterns cannot bind values",
                ));
            }
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, nia_ast::PatternKind::Wildcard)
                    && arm.patterns.len() != 1
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arm.span,
                        "`_` default must be the only pattern in a match arm",
                    ));
                }
                // Recursive checking still owns type compatibility, binding types, and
                // constant evaluation. A fresh local coverage value prevents the legacy
                // per-constructor accumulator from making cross-arm decisions.
                let mut local_coverage = PatternCoverage::default();
                self.check_pattern(
                    pattern,
                    target_ty,
                    Some(&mut local_coverage),
                    "match pattern",
                );
                let normalized = self.analysis_pattern(pattern, target_ty);
                match useful_witness(
                    &matrix,
                    std::slice::from_ref(&normalized),
                    &[target_ty],
                    |ty| self.analysis_domain(*ty),
                ) {
                    Ok(Some(_)) => matrix.push(vec![normalized]),
                    Ok(None) => self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        pattern.span,
                        "match pattern is unreachable because previous patterns cover all of its values",
                    )),
                    Err(error) => self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        pattern.span,
                        format!("cannot analyze match pattern coverage: {error}"),
                    )),
                }
            }
            let arm_ty = self.check_match_arm_body(&arm.body, result_ty);
            if let Some(expected) = result_ty {
                self.expect_match_arm_type(&arm.body, expected, arm_ty);
            } else if !self.is_never(arm_ty) {
                result_ty = Some(arm_ty);
            }
        }

        self.check_pattern_matrix_exhaustive(matched.target.span, target_ty, &matrix);
        result_ty.unwrap_or_else(|| self.unit())
    }

    pub(crate) fn check_if_pattern_expr(
        &mut self,
        if_pattern: &nia_ast::IfPatternExpr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&if_pattern.target);
        let mut coverage = PatternCoverage::default();
        let mut result_ty = expected;
        self.check_pattern(
            &if_pattern.pattern,
            target_ty,
            Some(&mut coverage),
            "if pattern",
        );
        let then_ty = self.check_block_with_expected(&if_pattern.then_branch, result_ty);
        if let Some(expected) = result_ty {
            self.expect_block_tail_type(
                &if_pattern.then_branch,
                expected,
                then_ty,
                "if pattern branches",
            );
        } else if !self.is_never(then_ty) {
            result_ty = Some(then_ty);
        }

        let Some(else_branch) = &if_pattern.else_branch else {
            if self.pattern_coverage_covers_type(target_ty, &coverage) {
                return result_ty.unwrap_or_else(|| self.unit());
            }
            if expected.is_some_and(|expected| !self.is_unit(expected)) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    if_pattern.target.span,
                    "non-exhaustive if pattern requires an `else` branch",
                ));
            }
            return self.unit();
        };
        let else_ty = self.check_expr_with_expected(else_branch, result_ty);
        if let Some(expected) = result_ty {
            self.expect_expr_or_block_tail_type(
                else_branch,
                expected,
                else_ty,
                "if pattern branches",
            );
            expected
        } else {
            else_ty
        }
    }

    fn check_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        target_ty: InternedTyId,
        coverage: Option<&mut PatternCoverage>,
        context: &str,
    ) {
        match &pattern.kind {
            nia_ast::PatternKind::Wildcard => {
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage.catch_all = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.local_def(node_key) {
                    self.record_local_type(local_id, target_ty);
                }
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage.catch_all = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let expected_readonly = matches!(pattern.kind, nia_ast::PatternKind::Pointer(_));
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Pointer { is_readonly, elem })
                        if *is_readonly == expected_readonly =>
                    {
                        *elem
                    }
                    Some(TyKind::Pointer { .. }) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} pointer mutability does not match target"),
                        ));
                        self.error()
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} pointer pattern requires a pointer target"),
                        ));
                        self.error()
                    }
                };
                self.check_pattern(inner, elem_ty, coverage, context);
            }
            nia_ast::PatternKind::OptionalSome(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Optional { elem }) => *elem,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`?` pattern requires an optional target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .optional_some
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, elem_ty, child_coverage, context);
            }
            nia_ast::PatternKind::OptionalNull => {
                if !matches!(
                    self.interner.get(self.normalization.normalize(target_ty)),
                    Some(TyKind::Optional { .. })
                ) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        pattern.span,
                        format!(
                            "`null` pattern requires an optional target, found `{}`",
                            self.ty_name(target_ty)
                        ),
                    ));
                }
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all.or(coverage.optional_null) {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage.optional_null = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::ErrorOk(inner) => {
                let value_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { value, .. }) => *value,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`!` pattern requires an error union target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .error_ok
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, value_ty, child_coverage, context);
            }
            nia_ast::PatternKind::ErrorErr(inner) => {
                let error_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { error, .. }) => *error,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`pattern!` requires an error union target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .error_err
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, error_ty, child_coverage, context);
            }
            nia_ast::PatternKind::Tuple(patterns) => {
                let elem_types = match self
                    .interner
                    .get(self.normalization.normalize(target_ty))
                    .cloned()
                {
                    Some(TyKind::Tuple(elems)) if elems.len() == patterns.len() => elems,
                    Some(TyKind::Tuple(elems)) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "{context} tuple arity mismatch: expected {}, found {}",
                                elems.len(),
                                patterns.len()
                            ),
                        ));
                        vec![self.error(); patterns.len()]
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} tuple pattern requires a tuple target"),
                        ));
                        vec![self.error(); patterns.len()]
                    }
                };
                let mut field_coverages = vec![PatternCoverage::default(); patterns.len()];
                for ((pattern, elem_ty), field_coverage) in patterns
                    .iter()
                    .zip(elem_types.iter().copied())
                    .zip(&mut field_coverages)
                {
                    self.check_pattern(pattern, elem_ty, Some(field_coverage), context);
                }
                if let Some(coverage) = coverage
                    && elem_types
                        .iter()
                        .copied()
                        .zip(&field_coverages)
                        .all(|(elem_ty, field)| self.pattern_coverage_covers_type(elem_ty, field))
                {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage.catch_all = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::Nominal {
                constructor,
                fields,
            } => {
                if self.enum_variant_info(constructor).is_some() {
                    self.check_enum_variant_pattern(
                        pattern.span,
                        constructor,
                        fields,
                        target_ty,
                        coverage,
                        context,
                    );
                } else {
                    self.check_struct_pattern(
                        pattern.span,
                        constructor,
                        fields,
                        target_ty,
                        coverage,
                        context,
                    );
                }
            }
            nia_ast::PatternKind::Expr(expr) => {
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    self.check_match_expr_pattern(
                        expr,
                        target_ty,
                        self.enum_global_def_id(target_ty),
                        context,
                        &mut coverage.enum_variants,
                        &mut coverage.intervals,
                    );
                } else {
                    let pattern_ty = self.check_expr_with_expected(expr, Some(target_ty));
                    self.expect_expr_type(expr, target_ty, pattern_ty, context);
                }
            }
            nia_ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                let range = RangePatternCheck {
                    span: pattern.span,
                    start,
                    end,
                    inclusive: *inclusive,
                };
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    self.check_match_range_pattern(
                        range,
                        target_ty,
                        context,
                        &mut coverage.intervals,
                    );
                } else {
                    self.check_if_pattern_range(range, target_ty, context);
                }
            }
        }
    }

    pub(super) fn check_irrefutable_struct_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        constructor: &Expr,
        fields: &nia_ast::NominalPatternFields,
        target_ty: InternedTyId,
        context: &str,
    ) -> InternedTyId {
        let mut coverage = PatternCoverage::default();
        self.check_struct_pattern(
            pattern.span,
            constructor,
            fields,
            target_ty,
            Some(&mut coverage),
            context,
        );
        if self.pattern_coverage_covers_type(target_ty, &coverage) {
            target_ty
        } else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} must be irrefutable"),
            ));
            self.error()
        }
    }

    fn check_struct_pattern(
        &mut self,
        span: Span,
        constructor: &Expr,
        fields: &nia_ast::NominalPatternFields,
        target_ty: InternedTyId,
        coverage: Option<&mut PatternCoverage>,
        context: &str,
    ) {
        let Some((constructor_def, _, _)) = self.type_prefix_instance(constructor) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                constructor.span,
                format!("{context} constructor is not a struct or enum variant"),
            ));
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        };
        let normalized = self.normalization.normalize(target_ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(normalized).cloned() else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("{context} struct pattern requires a struct target"),
            ));
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        };
        if self.is_enum_def(constructor_def)
            || self.is_union_def(constructor_def)
            || self.resolved_struct_signature(constructor_def).is_none()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                constructor.span,
                format!("{context} constructor is not a struct"),
            ));
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        }
        if constructor_def != def_id {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                constructor.span,
                format!(
                    "{context} constructor does not match target type `{}`",
                    self.ty_name(target_ty)
                ),
            ));
        }
        let nia_ast::NominalPatternFields::Named {
            fields: actual,
            rest,
        } = fields
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "struct patterns require named fields",
            ));
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        };
        let Some(signature) = self.resolved_struct_signature(constructor_def) else {
            return;
        };
        let expected = signature.signature.fields;
        let field_set = nia_sema::check_required_field_set(
            actual
                .iter()
                .map(|field| nia_sema::NamedField::new(field.span, field.name)),
            expected.iter().map(|field| field.name),
        );
        let field_set_is_valid = field_set.duplicate_fields.is_empty()
            && field_set.unknown_fields.is_empty()
            && (rest.is_some() || field_set.missing_fields.is_empty());
        let mut fields_are_irrefutable = true;

        // Check in declaration order. Typed lowering relies on this same canonical order so
        // source field order never changes either runtime projection or coverage semantics.
        for expected_field in &expected {
            let Some(field) = actual
                .iter()
                .find(|field| field.name == expected_field.name)
            else {
                // Omission is a wildcard only when the source explicitly wrote `..`.
                fields_are_irrefutable &= rest.is_some();
                continue;
            };
            let ty = self
                .field_ty_for_aggregate_ty(target_ty, &expected_field.name)
                .unwrap_or_else(|| self.error());
            let mut child = PatternCoverage::default();
            self.check_pattern(&field.pattern, ty, Some(&mut child), context);
            fields_are_irrefutable &= self.pattern_coverage_covers_type(ty, &child);
        }
        for field in &field_set.duplicate_fields {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!(
                    "duplicate struct pattern field `{}`",
                    self.symbol_name(field.name)
                ),
            ));
        }
        for field in &field_set.unknown_fields {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!(
                    "unknown struct pattern field `{}`",
                    self.symbol_name(field.name)
                ),
            ));
        }
        if rest.is_none() {
            for name in &field_set.missing_fields {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("missing struct pattern field `{}`", self.symbol_name(*name)),
                ));
            }
        }
        if field_set_is_valid
            && fields_are_irrefutable
            && let Some(coverage) = coverage
        {
            if let Some(previous) = coverage.catch_all {
                self.report_pattern_overlap(span, previous);
            }
            coverage.catch_all = Some(span);
        }
    }

    fn check_enum_variant_pattern(
        &mut self,
        span: Span,
        variant_expr: &Expr,
        fields: &nia_ast::NominalPatternFields,
        target_ty: InternedTyId,
        coverage: Option<&mut PatternCoverage>,
        context: &str,
    ) {
        let Some((enum_id, variant_def)) = self.enum_variant_info(variant_expr) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                variant_expr.span,
                format!("{context} payload target is not an enum variant"),
            ));
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        };
        let expected_enum = self.enum_global_def_id(target_ty);
        if expected_enum != Some(enum_id) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                variant_expr.span,
                format!(
                    "{context} variant belongs to a different enum than `{}`",
                    self.ty_name(target_ty)
                ),
            ));
        }
        self.record_expr_node_type(variant_expr, target_ty);
        let variant_id = GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_def,
        };
        let Some((_, variant)) = self.resolved_enum_variant(variant_id) else {
            self.check_invalid_enum_pattern_fields(fields, context);
            return;
        };
        let mut field_coverages = Vec::new();
        match (&variant.payload, fields) {
            (
                nia_item_signatures::EnumVariantPayloadSignature::Tuple(expected),
                nia_ast::NominalPatternFields::Tuple(actual),
            ) => {
                if expected.len() != actual.len() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!(
                            "enum variant `{}` expects {} pattern fields, found {}",
                            self.symbol_name(variant.name),
                            expected.len(),
                            actual.len()
                        ),
                    ));
                }
                if expected.len() == 1
                    && actual.len() == 1
                    && let Some(coverage) = coverage
                {
                    self.check_single_field_enum_payload_coverage(
                        variant_def,
                        span,
                        expected[0],
                        &actual[0],
                        coverage,
                        context,
                    );
                    return;
                }
                for (index, pattern) in actual.iter().enumerate() {
                    let ty = expected.get(index).copied().unwrap_or_else(|| self.error());
                    let mut child = PatternCoverage::default();
                    self.check_pattern(pattern, ty, Some(&mut child), context);
                    field_coverages.push((ty, child));
                }
            }
            (
                nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
                nia_ast::NominalPatternFields::Named {
                    fields: actual,
                    rest,
                },
            ) => {
                let field_set = nia_sema::check_required_field_set(
                    actual
                        .iter()
                        .map(|field| nia_sema::NamedField::new(field.span, field.name)),
                    expected.iter().map(|field| field.name),
                );
                if expected.len() == 1
                    && actual.len() == 1
                    && actual[0].name == expected[0].name
                    && let Some(coverage) = coverage
                {
                    self.check_single_field_enum_payload_coverage(
                        variant_def,
                        span,
                        expected[0].ty,
                        &actual[0].pattern,
                        coverage,
                        context,
                    );
                    return;
                }
                for field in actual {
                    let ty = expected
                        .iter()
                        .find(|expected| expected.name == field.name)
                        .map(|expected| expected.ty)
                        .unwrap_or_else(|| self.error());
                    let mut child = PatternCoverage::default();
                    self.check_pattern(&field.pattern, ty, Some(&mut child), context);
                    field_coverages.push((ty, child));
                }
                if rest.is_some() {
                    for missing in &field_set.missing_fields {
                        let ty = expected
                            .iter()
                            .find(|expected| expected.name == *missing)
                            .map(|expected| expected.ty)
                            .unwrap_or_else(|| self.error());
                        let child = PatternCoverage {
                            catch_all: *rest,
                            ..PatternCoverage::default()
                        };
                        field_coverages.push((ty, child));
                    }
                }
                for field in field_set.duplicate_fields {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        field.span,
                        format!(
                            "duplicate payload pattern field `{}`",
                            self.symbol_name(field.name)
                        ),
                    ));
                }
                for field in field_set.unknown_fields {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        field.span,
                        format!(
                            "unknown payload pattern field `{}`",
                            self.symbol_name(field.name)
                        ),
                    ));
                }
                if rest.is_none() {
                    for name in field_set.missing_fields {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            span,
                            format!("missing payload pattern field `{}`", self.symbol_name(name)),
                        ));
                    }
                }
            }
            (nia_item_signatures::EnumVariantPayloadSignature::Unit, _) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "unit enum variant `{}` has no payload",
                        self.symbol_name(variant.name)
                    ),
                ));
                self.check_invalid_enum_pattern_fields(fields, context);
            }
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "enum variant `{}` payload pattern has the wrong shape",
                        self.symbol_name(variant.name)
                    ),
                ));
                self.check_invalid_enum_pattern_fields(fields, context);
            }
        }
        let payload_is_complete = match &variant.payload {
            nia_item_signatures::EnumVariantPayloadSignature::Unit => false,
            nia_item_signatures::EnumVariantPayloadSignature::Tuple(expected) => {
                expected.len() == field_coverages.len()
                    && field_coverages
                        .iter()
                        .all(|(ty, coverage)| self.pattern_coverage_covers_type(*ty, coverage))
            }
            nia_item_signatures::EnumVariantPayloadSignature::Named(expected) => {
                expected.len() == field_coverages.len()
                    && field_coverages
                        .iter()
                        .all(|(ty, coverage)| self.pattern_coverage_covers_type(*ty, coverage))
            }
        };
        if payload_is_complete
            && let Some(coverage) = coverage
            && let Some(previous) = coverage.enum_variants.insert(variant_def, span)
        {
            self.report_pattern_overlap(span, previous);
        }
    }

    fn check_single_field_enum_payload_coverage(
        &mut self,
        variant_def: DefId,
        span: Span,
        field_ty: InternedTyId,
        pattern: &nia_ast::Pattern,
        coverage: &mut PatternCoverage,
        context: &str,
    ) {
        let mut field_coverage = coverage
            .single_field_enum_payloads
            .remove(&variant_def)
            .map(|(_, coverage)| coverage)
            .unwrap_or_default();
        self.check_pattern(pattern, field_ty, Some(&mut field_coverage), context);
        let complete = self.pattern_coverage_covers_type(field_ty, &field_coverage);
        coverage
            .single_field_enum_payloads
            .insert(variant_def, (field_ty, field_coverage));
        if complete {
            coverage.enum_variants.entry(variant_def).or_insert(span);
        }
    }

    fn check_invalid_enum_pattern_fields(
        &mut self,
        fields: &nia_ast::NominalPatternFields,
        context: &str,
    ) {
        match fields {
            nia_ast::NominalPatternFields::Tuple(fields) => {
                for field in fields {
                    let error = self.error();
                    self.check_pattern(field, error, None, context);
                }
            }
            nia_ast::NominalPatternFields::Named { fields, .. } => {
                for field in fields {
                    let error = self.error();
                    self.check_pattern(&field.pattern, error, None, context);
                }
            }
        }
    }

    fn check_if_pattern_range(
        &mut self,
        pattern: RangePatternCheck<'_>,
        target_ty: InternedTyId,
        context: &str,
    ) {
        if !self.is_integer(target_ty) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range requires an integer target"),
            ));
        }
        let start_ty = self.check_expr_with_expected(pattern.start, Some(target_ty));
        self.expect_expr_type(pattern.start, target_ty, start_ty, context);
        let end_ty = self.check_expr_with_expected(pattern.end, Some(target_ty));
        self.expect_expr_type(pattern.end, target_ty, end_ty, context);
    }

    fn report_pattern_overlap(&mut self, span: Span, previous: Span) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("pattern overlaps previous pattern at {previous:?}"),
        ));
    }

    fn pattern_coverage_covers_type(
        &mut self,
        target_ty: InternedTyId,
        coverage: &PatternCoverage,
    ) -> bool {
        if coverage.catch_all.is_some() {
            return true;
        }
        let normalized = self.normalization.normalize(target_ty);
        match self.interner.get(normalized).cloned() {
            Some(TyKind::Optional { elem }) => {
                coverage.optional_null.is_some()
                    && if let Some(coverage) = coverage.optional_some.as_deref() {
                        self.pattern_coverage_covers_type(elem, coverage)
                    } else {
                        false
                    }
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let ok_covered = if let Some(coverage) = coverage.error_ok.as_deref() {
                    self.pattern_coverage_covers_type(value, coverage)
                } else {
                    false
                };
                let err_covered = if let Some(coverage) = coverage.error_err.as_deref() {
                    self.pattern_coverage_covers_type(error, coverage)
                } else {
                    false
                };
                ok_covered && err_covered
            }
            _ if self.is_bool(target_ty) => self.pattern_intervals_cover_bool(&coverage.intervals),
            _ => self.enum_global_def_id(target_ty).is_some_and(|enum_id| {
                let Some(resolved) = self.resolved_enum_signature(enum_id) else {
                    return false;
                };
                !resolved.signature.is_open
                    && resolved
                        .signature
                        .variants
                        .iter()
                        .all(|variant| coverage.enum_variants.contains_key(&variant.def_id))
            }),
        }
    }

    fn pattern_intervals_cover_bool(&self, intervals: &[MatchInterval]) -> bool {
        let covers = |tag: i128| {
            intervals
                .iter()
                .any(|interval| interval.start <= tag && tag <= interval.end)
        };
        covers(0) && covers(1)
    }

    fn check_match_expr_pattern(
        &mut self,
        pattern: &Expr,
        target_ty: InternedTyId,
        enum_id: Option<GlobalDefId>,
        context: &str,
        covered_enum_variants: &mut HashMap<DefId, Span>,
        covered_intervals: &mut Vec<MatchInterval>,
    ) {
        let pattern_ty = self.check_expr_with_expected(pattern, Some(target_ty));
        if self.is_open_enum(target_ty)
            && self.check_integer_literal_enum_backing_range(pattern, target_ty, context)
        {
            self.record_expr_node_type(pattern, target_ty);
        } else {
            self.expect_expr_type(pattern, target_ty, pattern_ty, context);
        }
        if let Some(expected_enum) = enum_id
            && let Some((variant_enum, variant_id)) = self.enum_variant_info(pattern)
            && variant_enum == expected_enum
        {
            if let Some(previous) = covered_enum_variants.insert(variant_id, pattern.span) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} overlaps previous pattern at {previous:?}"),
                ));
            }
            return;
        }
        if self.is_integer(target_ty) || self.is_bool(target_ty) {
            let Some(value) = self.pattern_int_value(pattern) else {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} must be a compile-time integer constant"),
                ));
                return;
            };
            self.check_match_interval_overlap(
                MatchInterval {
                    start: value,
                    end: value,
                    span: pattern.span,
                },
                covered_intervals,
            );
        }
    }

    fn check_match_range_pattern(
        &mut self,
        pattern: RangePatternCheck<'_>,
        target_ty: InternedTyId,
        context: &str,
        covered_intervals: &mut Vec<MatchInterval>,
    ) {
        if !self.is_integer(target_ty) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range requires an integer target"),
            ));
        }
        let start_ty = self.check_expr_with_expected(pattern.start, Some(target_ty));
        self.expect_expr_type(pattern.start, target_ty, start_ty, context);
        let end_ty = self.check_expr_with_expected(pattern.end, Some(target_ty));
        self.expect_expr_type(pattern.end, target_ty, end_ty, context);
        let Some(start_value) = self.pattern_int_value(pattern.start) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.start.span,
                format!("{context} range start must be a compile-time integer constant"),
            ));
            return;
        };
        let Some(end_value) = self.pattern_int_value(pattern.end) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.end.span,
                format!("{context} range end must be a compile-time integer constant"),
            ));
            return;
        };
        let Some(end_inclusive) = (if pattern.inclusive {
            Some(end_value)
        } else {
            end_value.checked_sub(1)
        }) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range endpoint is out of range"),
            ));
            return;
        };
        if start_value > end_inclusive {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range is empty"),
            ));
            return;
        }
        self.check_match_interval_overlap(
            MatchInterval {
                start: start_value,
                end: end_inclusive,
                span: pattern.span,
            },
            covered_intervals,
        );
    }

    fn pattern_int_value(&mut self, expr: &Expr) -> Option<i128> {
        let value = if let ExprKind::Bool(value) = expr.kind {
            if value { 1 } else { 0 }
        } else {
            match self
                .with_const_context(|this| {
                    let expr =
                        this.lower_const_expr(expr)
                            .map_err(|err| nia_const_eval::ConstError {
                                span: err.span,
                                message: err.message,
                            })?;
                    nia_const_eval::eval_resolved_const_expr(&expr, this)
                })
                .ok()?
            {
                nia_const_eval::ConstValue::Int(value) => value.as_i128()?,
                _ => return None,
            }
        };
        self.record_pattern_value(expr, value);
        Some(value)
    }

    fn check_match_interval_overlap(
        &mut self,
        interval: MatchInterval,
        covered_intervals: &mut Vec<MatchInterval>,
    ) {
        if let Some(previous) = covered_intervals
            .iter()
            .find(|previous| interval.start <= previous.end && previous.start <= interval.end)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                interval.span,
                format!(
                    "match pattern overlaps previous pattern at {:?}",
                    previous.span
                ),
            ));
        }
        covered_intervals.push(interval);
    }

    pub(super) fn is_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn check_match_arm_body(
        &mut self,
        body: &nia_ast::MatchArmBody,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match body {
            nia_ast::MatchArmBody::Expr(expr) => self.check_expr_with_expected(expr, expected),
            nia_ast::MatchArmBody::Stmt(stmt) => {
                self.check_stmt(stmt);
                if matches!(
                    stmt.kind,
                    StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
                ) {
                    self.never()
                } else {
                    self.unit()
                }
            }
            nia_ast::MatchArmBody::Block(block) => self.check_block_with_expected(block, expected),
        }
    }

    fn expect_match_arm_type(
        &mut self,
        body: &nia_ast::MatchArmBody,
        expected: InternedTyId,
        actual: InternedTyId,
    ) {
        if self.is_never(actual) {
            return;
        }
        match body {
            nia_ast::MatchArmBody::Expr(expr) => {
                self.expect_expr_type(expr, expected, actual, "match arms");
            }
            nia_ast::MatchArmBody::Block(block) => {
                self.expect_block_tail_type(block, expected, actual, "match arms");
            }
            nia_ast::MatchArmBody::Stmt(stmt) => {
                self.expect_type(stmt.span, expected, actual, "match arms");
            }
        }
    }
}
