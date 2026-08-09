// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SwitchInterval {
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
    intervals: Vec<SwitchInterval>,
    enum_variants: HashMap<DefId, Span>,
    single_field_enum_payloads: HashMap<DefId, (InternedTyId, Box<PatternCoverage>)>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_switch_expr(
        &mut self,
        switch: &nia_ast::SwitchStmt,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&switch.target);
        let mut coverage = PatternCoverage::default();
        let mut result_ty = expected;

        for arm in &switch.arms {
            if coverage.catch_all.is_some() {
                self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                    arm.span,
                    "switch arm is unreachable because a previous pattern matches all remaining values",
                ));
            }
            if arm.patterns.len() > 1 && arm.patterns.iter().any(nia_ast::Pattern::contains_binding)
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    arm.span,
                    "switch arms with multiple alternative patterns cannot bind values",
                ));
            }
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, nia_ast::PatternKind::Wildcard)
                    && arm.patterns.len() != 1
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arm.span,
                        "`_` default must be the only pattern in a switch arm",
                    ));
                }
                self.check_pattern(pattern, target_ty, Some(&mut coverage), "switch pattern");
            }
            let arm_ty = self.check_switch_arm_body(&arm.body, result_ty);
            if let Some(expected) = result_ty {
                self.expect_switch_arm_type(&arm.body, expected, arm_ty);
            } else if !self.is_never(arm_ty) {
                result_ty = Some(arm_ty);
            }
        }

        self.check_pattern_switch_exhaustive(switch.target.span, target_ty, &coverage);
        result_ty.unwrap_or_else(|| self.void())
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
                return result_ty.unwrap_or_else(|| self.void());
            }
            if expected.is_some_and(|expected| !self.is_void(expected)) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    if_pattern.target.span,
                    "non-exhaustive if pattern requires an `else` branch",
                ));
            }
            return self.void();
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
            nia_ast::PatternKind::EnumVariant { variant, fields } => {
                self.check_enum_variant_pattern(
                    pattern.span,
                    variant,
                    fields,
                    target_ty,
                    coverage,
                    context,
                );
            }
            nia_ast::PatternKind::Expr(expr) => {
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    self.check_switch_expr_pattern(
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
                    self.check_switch_range_pattern(
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

    fn check_enum_variant_pattern(
        &mut self,
        span: Span,
        variant_expr: &Expr,
        fields: &nia_ast::EnumVariantPatternFields,
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
                nia_ast::EnumVariantPatternFields::Tuple(actual),
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
                nia_ast::EnumVariantPatternFields::Named(actual),
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
                for name in field_set.missing_fields {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!("missing payload pattern field `{}`", self.symbol_name(name)),
                    ));
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
        fields: &nia_ast::EnumVariantPatternFields,
        context: &str,
    ) {
        match fields {
            nia_ast::EnumVariantPatternFields::Tuple(fields) => {
                for field in fields {
                    let error = self.error();
                    self.check_pattern(field, error, None, context);
                }
            }
            nia_ast::EnumVariantPatternFields::Named(fields) => {
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

    fn check_pattern_switch_exhaustive(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        coverage: &PatternCoverage,
    ) {
        if self.pattern_coverage_covers_type(target_ty, coverage) {
            return;
        }
        if let Some(enum_id) = self.enum_global_def_id(target_ty) {
            let covered = coverage.enum_variants.keys().copied().collect();
            self.check_enum_switch_exhaustive(span, enum_id, false, &covered);
            return;
        }
        if matches!(
            self.interner.get(self.normalization.normalize(target_ty)),
            Some(TyKind::Optional { .. } | TyKind::ErrorUnion { .. })
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "non-exhaustive switch over destructured value",
            ));
        }
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

    fn pattern_intervals_cover_bool(&self, intervals: &[SwitchInterval]) -> bool {
        let covers = |tag: i128| {
            intervals
                .iter()
                .any(|interval| interval.start <= tag && tag <= interval.end)
        };
        covers(0) && covers(1)
    }

    fn check_switch_expr_pattern(
        &mut self,
        pattern: &Expr,
        target_ty: InternedTyId,
        enum_id: Option<GlobalDefId>,
        context: &str,
        covered_enum_variants: &mut HashMap<DefId, Span>,
        covered_intervals: &mut Vec<SwitchInterval>,
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
            self.check_switch_interval_overlap(
                SwitchInterval {
                    start: value,
                    end: value,
                    span: pattern.span,
                },
                covered_intervals,
            );
        }
    }

    fn check_switch_range_pattern(
        &mut self,
        pattern: RangePatternCheck<'_>,
        target_ty: InternedTyId,
        context: &str,
        covered_intervals: &mut Vec<SwitchInterval>,
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
        self.check_switch_interval_overlap(
            SwitchInterval {
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

    fn check_switch_interval_overlap(
        &mut self,
        interval: SwitchInterval,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        if let Some(previous) = covered_intervals
            .iter()
            .find(|previous| interval.start <= previous.end && previous.start <= interval.end)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                interval.span,
                format!(
                    "switch pattern overlaps previous pattern at {:?}",
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

    fn check_switch_arm_body(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => self.check_expr_with_expected(expr, expected),
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.check_stmt(stmt);
                if matches!(
                    stmt.kind,
                    StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
                ) {
                    self.never()
                } else {
                    self.void()
                }
            }
            nia_ast::SwitchArmBody::Block(block) => self.check_block_with_expected(block, expected),
        }
    }

    fn expect_switch_arm_type(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: InternedTyId,
        actual: InternedTyId,
    ) {
        if self.is_never(actual) {
            return;
        }
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => {
                self.expect_expr_type(expr, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Block(block) => {
                self.expect_block_tail_type(block, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.expect_type(stmt.span, expected, actual, "switch arms");
            }
        }
    }
}
