// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{float_literal_suffix_ty, integer_literal_suffix_ty};
use nia_ast::{AssignOp, BinaryOp, BracketArg, ComptimeIfExpr, Expr, ExprKind, IndexArg, UnaryOp};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_local_resolve::LocalUse;
use nia_sema_ir::{BracketSuffixResolution, BuiltinOperatorOp, ComptimeIfSelection};
use nia_span::Span;
use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, RangeTyKind, TraitId, TyKind};
use nia_value_resolve::ValueNameResolution;

struct BuiltinOperatorFinish<'a> {
    span: Span,
    trait_id: BuiltinTrait,
    lhs: &'a Expr,
    lhs_actual: InternedTyId,
    rhs: &'a Expr,
    rhs_actual: InternedTyId,
    expected: Option<InternedTyId>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_expr(&mut self, expr: &Expr) -> InternedTyId {
        self.check_expr_with_expected(expr, None)
    }

    pub(crate) fn check_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        if let Some(ty) = expected
            .and_then(|expected| self.expected_comptime_expr_runtime_projection(expr, expected))
        {
            self.record_expr_node_type(expr, ty);
            return ty;
        }
        let array_expected = self.array_expected_from_slice_expected(expected);
        let ty = match &expr.kind {
            ExprKind::Error | ExprKind::Raw(_) => self.error(),
            ExprKind::Integer(_) => self.integer_literal_type(expr),
            ExprKind::Float(_) => self.float_literal_type(expr),
            ExprKind::String(text) => self.string_literal_type(text),
            ExprKind::ByteString(text) => self.byte_string_literal_type(text),
            ExprKind::CString(text) => self.c_string_literal_type(text),
            ExprKind::Char(_) => self.primitive(PrimitiveTy::Char),
            ExprKind::ByteChar(_) => self.primitive(PrimitiveTy::U8),
            ExprKind::Bool(_) => self.bool(),
            ExprKind::Null => self.check_null_expr(expr.span, expected),
            ExprKind::Underscore => self.error(),
            ExprKind::Ident(_) => self.ident_type(expr),
            ExprKind::Builtin { name, type_arg } => self.check_builtin(expr, name, type_arg),
            ExprKind::TypeTarget { .. } => self.error(),
            ExprKind::BracketSuffix { callee, args } => {
                self.check_bracket_suffix_expr(expr, callee, args, expected)
            }
            ExprKind::ArrayLiteral { elems } => {
                self.check_array_literal(expr.span, array_expected.or(expected), elems)
            }
            ExprKind::StructLiteral { fields } => {
                self.check_struct_literal(expr.span, expected, fields)
            }
            ExprKind::TypedArrayLiteral { ty, elems } => {
                let explicit = self.ty_for_type(ty);
                self.check_array_literal(expr.span, Some(explicit), elems)
            }
            ExprKind::TypedStructLiteral { ty, fields } => {
                let explicit = self.ty_for_type(ty);
                self.check_struct_literal(expr.span, Some(explicit), fields)
            }
            ExprKind::Unary { op, expr: inner } => {
                let expected_ref_target = match (op, expected.and_then(|ty| self.interner.get(ty)))
                {
                    (
                        UnaryOp::RefReadOnly,
                        Some(TyKind::Pointer {
                            is_readonly: true,
                            elem,
                        }),
                    )
                    | (
                        UnaryOp::Ref,
                        Some(TyKind::Pointer {
                            is_readonly: false,
                            elem,
                        }),
                    ) => Some(*elem),
                    _ => None,
                };
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && let Some(function_ptr_ty) =
                        self.check_function_ref(inner, matches!(op, UnaryOp::RefReadOnly))
                {
                    self.record_expr_node_type(expr, function_ptr_ty);
                    return function_ptr_ty;
                }
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                {
                    match op {
                        UnaryOp::RefReadOnly => {
                            let slice_ty =
                                self.check_slice_ref(expr.span, lhs, range, true, expected);
                            self.record_expr_node_type(inner, slice_ty);
                            self.record_expr_node_type(expr, slice_ty);
                            return slice_ty;
                        }
                        UnaryOp::Ref => {
                            let slice_ty =
                                self.check_slice_ref(expr.span, lhs, range, false, expected);
                            self.record_expr_node_type(inner, slice_ty);
                            self.record_expr_node_type(expr, slice_ty);
                            return slice_ty;
                        }
                        _ => {}
                    }
                }
                let inner_ty = self.check_expr_with_expected(inner, expected_ref_target);
                match op {
                    UnaryOp::Neg | UnaryOp::Not | UnaryOp::BitNot => {
                        self.check_builtin_unary_operator_expr(expr.span, *op, inner, inner_ty)
                    }
                    UnaryOp::RefReadOnly => {
                        if self.is_invalid_temporary_type(inner_ty) {
                            self.diagnostics.push(Diagnostic::error(
                                inner.span,
                                "reference target cannot have void or never type",
                            ));
                        }
                        self.check_reference_target(inner, "reference target", true);
                        self.interner.intern(TyKind::Pointer {
                            is_readonly: true,
                            elem: inner_ty,
                        })
                    }
                    UnaryOp::Ref => {
                        if self.is_invalid_temporary_type(inner_ty) {
                            self.diagnostics.push(Diagnostic::error(
                                inner.span,
                                "reference target cannot have void or never type",
                            ));
                        }
                        self.check_reference_target(inner, "reference target", false);
                        self.interner.intern(TyKind::Pointer {
                            is_readonly: false,
                            elem: inner_ty,
                        })
                    }
                    UnaryOp::Deref => self.deref_result_type(expr.span, inner_ty),
                }
            }
            ExprKind::OptionalSome { expr: inner } => {
                self.check_optional_some_expr(inner, expected)
            }
            ExprKind::ErrorOk { expr: inner } => self.check_error_ok_expr(inner, expected),
            ExprKind::ErrorErr { expr: inner } => self.check_error_err_expr(inner, expected),
            ExprKind::Try { expr: inner } => self.check_try_expr(expr.span, inner),
            ExprKind::Binary { lhs, op, rhs } => {
                self.check_binary_expr(expr.span, lhs, *op, rhs, expected)
            }
            ExprKind::Assign { lhs, op, rhs } => {
                if matches!(lhs.kind, ExprKind::Underscore) {
                    self.check_expr(rhs);
                    if !matches!(op, AssignOp::Assign) {
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            "`_` discard only supports plain assignment",
                        ));
                    }
                    self.void()
                } else {
                    self.check_assignment_lhs(lhs);
                    self.check_assignable(lhs, "assignment target");
                    let lhs_ty = self.assignable_expr_type(lhs);
                    let rhs_ty = self.check_expr_with_expected(rhs, Some(lhs_ty));
                    self.expect_expr_type(rhs, lhs_ty, rhs_ty, "assignment");
                    self.void()
                }
            }
            ExprKind::Cast { expr: inner, ty } => {
                let source = self.check_expr(inner);
                let target = self.ty_for_type(ty);
                self.check_cast(expr.span, source, target);
                if self.is_open_enum(target) {
                    self.check_integer_literal_enum_backing_range(inner, target, "cast");
                }
                target
            }
            ExprKind::Call { callee, args } => self.check_call(expr, callee, args, expected),
            ExprKind::Field { lhs, name } => self.check_field_access(expr, lhs, name),
            ExprKind::Qualified { lhs, name } => {
                if let Some(ty) = self.check_enum_variant_access(expr.span, lhs, name) {
                    ty
                } else if self
                    .values
                    .node_qualified_values
                    .contains_key(&expr.node_key)
                {
                    self.qualified_global_type(expr)
                        .unwrap_or_else(|| self.error())
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "qualified access is not a value expression",
                    ));
                    self.error()
                }
            }
            ExprKind::Index { lhs, index } => {
                if let IndexArg::Expr(index) = index
                    && let Some(ty) = self.comptime_index_expr_runtime_type(lhs, index)
                {
                    self.check_expr(index);
                    return ty;
                }
                if matches!(index, IndexArg::Range(_))
                    && self.in_comptime_context()
                    && let Some(ty) = self.comptime_slice_expr_runtime_type(expr, expected)
                {
                    return ty;
                }
                let lhs_expected = match index {
                    IndexArg::Expr(_) => self.array_expected_from_index_expected(expected),
                    IndexArg::Range(_) => None,
                };
                let lhs_ty = self.check_expr_with_expected(lhs, lhs_expected);
                match index {
                    IndexArg::Expr(index) => {
                        let index_ty =
                            self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::IndexRead, index);
                        self.expect_integer(index.span, index_ty, "index");
                        let index_ty = self.expr_ty(index).unwrap_or(index_ty);
                        if index_ty == self.error() {
                            return self.error();
                        }
                        self.index_result_type_for_index(expr.span, lhs_ty, index_ty)
                    }
                    IndexArg::Range(range) => {
                        self.check_slice_range_bounds(range);
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            "range index expression must be borrowed as a slice; use `&base[..]` or `&mut base[..]`",
                        ));
                        self.slice_result_type(lhs_ty, false)
                    }
                }
            }
            ExprKind::Range(range) => self.check_range_expr(expr.span, range, expected),
            ExprKind::Block(block) => self.check_block_with_expected(block, expected),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.check_if_expr(cond, then_branch, else_branch.as_deref(), expected),
            ExprKind::ComptimeIf(comptime_if) => {
                self.check_comptime_if_expr(expr, comptime_if, expected)
            }
            ExprKind::Switch(switch) => self.check_switch_expr(switch, expected),
        };
        let ty = if let Some(expected) = expected {
            self.coerce_c_string_to_pointer(expr, expected, ty)
                .or_else(|| self.coerce_array_to_slice(expr, expected, ty))
                .or_else(|| self.coerce_trait_object_to_supertrait(expr, expected, ty))
                .or_else(|| self.coerce_pointer_to_trait_object(expr, expected, ty))
                .or_else(|| self.coerce_value_to_trait_object(expr, expected, ty))
                .or_else(|| self.materialize_inferred_array_type(expected, ty))
                .unwrap_or(ty)
        } else {
            ty
        };
        self.record_expr_node_type(expr, ty);
        ty
    }

    fn expected_comptime_expr_runtime_projection(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        let comptime_expr = self.lower_comptime_expr(expr).ok()?;
        match self.comptime_expr_type_for_ir_with_expected(&comptime_expr, Some(expected))? {
            nia_comptime_check::ComptimeValueType::String
                if self.is_runtime_char_array_type(expected) =>
            {
                Some(expected)
            }
            _ => None,
        }
    }

    fn is_runtime_char_array_type(&self, ty: InternedTyId) -> bool {
        let Some(TyKind::Array { elem, .. }) = self.interner.get(ty) else {
            return false;
        };
        matches!(
            self.interner.get(*elem),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn check_null_expr(&mut self, span: Span, expected: Option<InternedTyId>) -> InternedTyId {
        let Some(expected) = expected.map(|ty| self.normalization.normalize(ty)) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "`null` requires an expected optional type",
            ));
            return self.error();
        };
        if matches!(self.interner.get(expected), Some(TyKind::Optional { .. })) {
            expected
        } else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "`null` requires an optional type, found `{}`",
                    self.ty_name(expected)
                ),
            ));
            self.error()
        }
    }

    fn check_optional_some_expr(
        &mut self,
        inner: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let expected_elem = expected.and_then(|expected| match self.interner.get(expected) {
            Some(TyKind::Optional { elem }) => Some(*elem),
            _ => None,
        });
        let elem = self.check_expr_with_expected(inner, expected_elem);
        self.interner.intern(TyKind::Optional { elem })
    }

    fn check_error_ok_expr(
        &mut self,
        inner: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let Some((error, value)) = expected.and_then(|expected| self.error_union_parts(expected))
        else {
            self.diagnostics.push(Diagnostic::error(
                inner.span,
                "`!value` requires an expected error union type",
            ));
            self.check_expr(inner);
            return self.error();
        };
        let actual = self.check_expr_with_expected(inner, Some(value));
        self.expect_expr_type(inner, value, actual, "error-union success value");
        self.interner.intern(TyKind::ErrorUnion { error, value })
    }

    fn check_error_err_expr(
        &mut self,
        inner: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let Some((error, value)) = expected.and_then(|expected| self.error_union_parts(expected))
        else {
            self.diagnostics.push(Diagnostic::error(
                inner.span,
                "`error!` requires an expected error union type",
            ));
            self.check_expr(inner);
            return self.error();
        };
        let actual = self.check_expr_with_expected(inner, Some(error));
        self.expect_expr_type(inner, error, actual, "error-union error value");
        self.interner.intern(TyKind::ErrorUnion { error, value })
    }

    fn check_try_expr(&mut self, span: Span, inner: &Expr) -> InternedTyId {
        let inner_ty = self.check_expr(inner);
        let normalized = self.normalization.normalize(inner_ty);
        match self.interner.get(normalized).cloned() {
            Some(TyKind::Optional { elem }) => {
                if !matches!(
                    self.interner
                        .get(self.normalization.normalize(self.current_return)),
                    Some(TyKind::Optional { .. })
                ) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "optional propagation requires an optional function return type",
                    ));
                }
                elem
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                match self.error_union_parts(self.current_return) {
                    Some((return_error, _)) => {
                        if !self.types_match(error, return_error) {
                            self.diagnostics.push(Diagnostic::error(
                                span,
                                format!(
                                    "error propagation type mismatch: cannot propagate `{}` from function returning `{}`",
                                    self.ty_name(error),
                                    self.ty_name(self.current_return)
                                ),
                            ));
                        }
                    }
                    None => {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            "error propagation requires an error union function return type",
                        ));
                    }
                }
                value
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "`.?` requires optional or error union operand, found `{}`",
                        self.ty_name(inner_ty)
                    ),
                ));
                self.error()
            }
        }
    }

    fn check_range_expr(
        &mut self,
        span: Span,
        range: &nia_ast::SliceRange,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let expected = expected.and_then(|expected| self.expected_range_parts(expected));
        let start_expected = expected.and_then(|expected| expected.bound);
        let end_expected = expected.and_then(|expected| expected.bound);
        let start_ty = range.start.as_ref().map(|start| {
            let actual = self.check_expr_with_expected(start, start_expected);
            if let Some(expected) = start_expected {
                self.expect_expr_type(start, expected, actual, "range start");
            }
            self.expr_ty(start).unwrap_or(actual)
        });
        let end_ty = range.end.as_ref().map(|end| {
            let actual = self.check_expr_with_expected(end, end_expected);
            if let Some(expected) = end_expected {
                self.expect_expr_type(end, expected, actual, "range end");
            }
            self.expr_ty(end).unwrap_or(actual)
        });
        for (ty, context) in [(start_ty, "range start"), (end_ty, "range end")] {
            if let Some(ty) = ty {
                self.expect_integer(span, ty, context);
            }
        }
        let kind = match (range.start.is_some(), range.end.is_some(), range.inclusive) {
            (true, true, false) => RangeTyKind::Exclusive,
            (true, true, true) => RangeTyKind::Inclusive,
            (true, false, false) => RangeTyKind::From,
            (false, true, false) => RangeTyKind::To,
            (false, true, true) => RangeTyKind::ToInclusive,
            (false, false, false) => RangeTyKind::Full,
            (true, false, true) | (false, false, true) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "inclusive range expression requires an end bound",
                ));
                return self.error();
            }
        };
        if let Some(expected) = expected
            && expected.kind != kind
        {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "range kind mismatch: expected {}, got {}",
                    self.ty_name(expected.ty),
                    self.range_kind_name(kind)
                ),
            ));
            return self.error();
        }
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) => {
                self.expect_type(span, start_ty, end_ty, "range bounds");
                Some(start_ty)
            }
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => None,
        };
        self.interner.intern(TyKind::Range { kind, bound })
    }

    fn expected_range_parts(&self, expected: InternedTyId) -> Option<ExpectedRangeParts> {
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Range { kind, bound }) = self.interner.get(expected) else {
            return None;
        };
        Some(ExpectedRangeParts {
            ty: expected,
            kind: *kind,
            bound: *bound,
        })
    }

    fn range_kind_name(&self, kind: RangeTyKind) -> &'static str {
        match kind {
            RangeTyKind::Exclusive => "`T..T`",
            RangeTyKind::Inclusive => "`T..=T`",
            RangeTyKind::From => "`T..`",
            RangeTyKind::To => "`..T`",
            RangeTyKind::ToInclusive => "`..=T`",
            RangeTyKind::Full => "`..`",
        }
    }

    fn integer_literal_type(&mut self, expr: &Expr) -> InternedTyId {
        if let Some(primitive) = integer_literal_suffix_ty(expr) {
            let ty = self.primitive(primitive);
            self.check_integer_literal_range(expr, ty, "literal suffix");
            return ty;
        }
        if self.numeric_literal_has_suffix(expr) {
            self.report_invalid_numeric_literal_suffix(expr, "integer");
            return self.error();
        }
        self.i32()
    }

    fn float_literal_type(&mut self, expr: &Expr) -> InternedTyId {
        if let Some(primitive) = float_literal_suffix_ty(expr) {
            let ty = self.primitive(primitive);
            self.check_float_literal_target(expr, ty, "literal suffix");
            return ty;
        }
        if self.numeric_literal_has_suffix(expr) {
            self.report_invalid_numeric_literal_suffix(expr, "float");
            return self.error();
        }
        self.f64()
    }

    fn check_if_expr(
        &mut self,
        cond: &Expr,
        then_branch: &nia_ast::Block,
        else_branch: Option<&Expr>,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let cond_ty = self.check_expr(cond);
        self.expect_type(cond.span, self.bool(), cond_ty, "if condition");
        let Some(else_branch) = else_branch else {
            self.check_block(then_branch);
            return self.void();
        };

        if let Some(expected) = expected {
            let then_ty = self.check_block_with_expected(then_branch, Some(expected));
            self.expect_block_tail_type(then_branch, expected, then_ty, "if branches");
            let else_ty = self.check_expr_with_expected(else_branch, Some(expected));
            self.expect_expr_or_block_tail_type(else_branch, expected, else_ty, "if branches");
            return expected;
        }

        if self.block_tail_is_numeric_literal(then_branch)
            && !self.is_numeric_literal_expr(else_branch)
        {
            let else_ty = self.check_expr(else_branch);
            let then_ty = self.check_block_with_expected(then_branch, Some(else_ty));
            self.expect_block_tail_type(then_branch, else_ty, then_ty, "if branches");
            return if self.is_never(then_ty) {
                else_ty
            } else {
                self.block_tail_materialized_type(then_branch, then_ty)
            };
        }

        let then_ty = self.check_block(then_branch);
        let else_ty = self.check_expr_with_expected(else_branch, Some(then_ty));
        self.expect_expr_type(else_branch, then_ty, else_ty, "if branches");
        if self.is_never(then_ty) {
            self.expr_ty(else_branch).unwrap_or(else_ty)
        } else {
            then_ty
        }
    }

    fn check_comptime_if_expr(
        &mut self,
        expr: &Expr,
        comptime_if: &ComptimeIfExpr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match self.with_comptime_context(|this| {
            let cond = this.lower_comptime_expr(&comptime_if.cond).map_err(|err| {
                nia_comptime_engine::ComptimeError {
                    span: err.span,
                    message: err.message,
                }
            })?;
            nia_comptime_engine::eval_resolved_comptime_bool_expr(&cond, this)
        }) {
            Ok(true) => {
                self.node_comptime_if_selections
                    .insert(expr.node_key.clone(), ComptimeIfSelection::Then);
                self.check_block_with_expected(&comptime_if.then_branch, expected)
            }
            Ok(false) => {
                let Some(else_branch) = comptime_if.else_branch.as_deref() else {
                    self.node_comptime_if_selections
                        .insert(expr.node_key.clone(), ComptimeIfSelection::None);
                    return self.void();
                };
                self.node_comptime_if_selections
                    .insert(expr.node_key.clone(), ComptimeIfSelection::Else);
                self.check_expr_with_expected(else_branch, expected)
            }
            Err(err) => {
                self.diagnostics
                    .push(Diagnostic::error(err.span, err.message));
                self.node_comptime_if_selections
                    .insert(expr.node_key.clone(), ComptimeIfSelection::None);
                self.error()
            }
        }
    }

    pub(crate) fn expect_block_tail_type(
        &mut self,
        block: &nia_ast::Block,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if let Some(tail) = block.tail.as_deref() {
            self.expect_expr_type(tail, expected, actual, context);
        } else {
            self.expect_type(block.span, expected, actual, context);
        }
    }

    fn expect_expr_or_block_tail_type(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if let ExprKind::Block(block) = &expr.kind {
            self.expect_block_tail_type(block, expected, actual, context);
        } else {
            self.expect_expr_type(expr, expected, actual, context);
        }
    }

    fn block_tail_materialized_type(
        &self,
        block: &nia_ast::Block,
        fallback: InternedTyId,
    ) -> InternedTyId {
        block
            .tail
            .as_deref()
            .and_then(|tail| self.expr_ty(tail))
            .unwrap_or(fallback)
    }

    fn block_tail_is_numeric_literal(&self, block: &nia_ast::Block) -> bool {
        block
            .tail
            .as_deref()
            .is_some_and(|tail| self.is_numeric_literal_expr(tail))
    }

    fn array_expected_from_index_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = expected?;
        Some(self.interner.intern(TyKind::Array {
            len: ArrayLenTy::Infer,
            elem: expected,
        }))
    }

    fn check_binary_expr(
        &mut self,
        span: Span,
        lhs: &Expr,
        op: BinaryOp,
        rhs: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match op {
            BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne => {
                self.check_builtin_operator_expr(span, lhs, op, rhs, Some(self.bool()))
            }
            BinaryOp::And | BinaryOp::Or => {
                let expected = self.bool();
                let lhs_ty = self.check_expr_with_expected(lhs, Some(expected));
                self.expect_expr_type(lhs, expected, lhs_ty, "logical operator");
                let rhs_ty = self.check_expr_with_expected(rhs, Some(expected));
                self.expect_expr_type(rhs, expected, rhs_ty, "logical operator");
                self.bool()
            }
            BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => {
                self.check_builtin_operator_expr(span, lhs, op, rhs, expected)
            }
            BinaryOp::Shl | BinaryOp::Shr => {
                self.check_builtin_operator_expr(span, lhs, op, rhs, expected)
            }
            _ => self.check_builtin_operator_expr(span, lhs, op, rhs, expected),
        }
    }

    fn check_builtin_operator_expr(
        &mut self,
        span: Span,
        lhs: &Expr,
        op: BinaryOp,
        rhs: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let Some(trait_id) = BuiltinOperatorOp::Binary(op).trait_id() else {
            return self.error();
        };
        let output_is_bool = builtin_trait_output_is_bool(trait_id);
        if output_is_bool && self.is_numeric_literal_expr(lhs) && !self.is_numeric_literal_expr(rhs)
        {
            let rhs_actual = self.check_expr(rhs);
            let rhs_ty = self.expr_ty(rhs).unwrap_or(rhs_actual);
            let lhs_actual = self.check_expr_with_expected(lhs, Some(rhs_ty));
            self.expect_expr_type(lhs, rhs_ty, lhs_actual, "binary operator");
            return self.finish_builtin_operator_expr(BuiltinOperatorFinish {
                span,
                trait_id,
                lhs,
                lhs_actual,
                rhs,
                rhs_actual,
                expected,
            });
        }

        let lhs_expected = (!output_is_bool).then_some(()).and_then(|_| {
            expected.filter(|expected| self.can_expected_type_drive_builtin_operator(*expected, op))
        });
        let lhs_actual = if let Some(expected) = lhs_expected {
            self.check_expr_with_expected(lhs, Some(expected))
        } else {
            self.check_expr(lhs)
        };
        if let Some(expected) = lhs_expected {
            self.expect_expr_type(lhs, expected, lhs_actual, "binary operator");
        }
        let lhs_ty = self.expr_ty(lhs).unwrap_or(lhs_actual);
        let rhs_expected = if self.is_numeric_literal_expr(rhs) {
            Some(lhs_ty)
        } else {
            None
        };
        let rhs_actual = self.check_expr_with_expected(rhs, rhs_expected);
        if let Some(expected) = rhs_expected {
            self.expect_expr_type(rhs, expected, rhs_actual, "binary operator");
        }
        self.finish_builtin_operator_expr(BuiltinOperatorFinish {
            span,
            trait_id,
            lhs,
            lhs_actual,
            rhs,
            rhs_actual,
            expected,
        })
    }

    fn finish_builtin_operator_expr(&mut self, finish: BuiltinOperatorFinish<'_>) -> InternedTyId {
        let lhs_ty = self.expr_ty(finish.lhs).unwrap_or(finish.lhs_actual);
        let rhs_ty = self.expr_ty(finish.rhs).unwrap_or(finish.rhs_actual);

        let trait_args = vec![rhs_ty];
        if !self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(finish.trait_id),
            trait_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                finish.span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(finish.trait_id, &trait_args)
                ),
            ));
        }

        let output = if builtin_trait_output_is_bool(finish.trait_id) {
            self.bool()
        } else {
            let output = self.interner.intern(TyKind::Projection {
                self_ty: lhs_ty,
                trait_id: TraitId::Builtin(finish.trait_id),
                trait_args,
                name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
            });
            self.normalize_projection(output)
        };
        if let Some(expected) = finish.expected {
            self.expect_type(finish.span, expected, output, "binary operator");
        }
        output
    }

    fn check_builtin_unary_operator_expr(
        &mut self,
        span: Span,
        op: UnaryOp,
        inner: &Expr,
        inner_ty: InternedTyId,
    ) -> InternedTyId {
        let Some(trait_id) = BuiltinOperatorOp::Unary(op).trait_id() else {
            return self.error();
        };
        let inner_ty = self.expr_ty(inner).unwrap_or(inner_ty);
        if !self.current_context_proves_trait_obligation(
            inner_ty,
            TraitId::Builtin(trait_id),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(inner_ty),
                    self.builtin_trait_ty_name(trait_id, &[])
                ),
            ));
        }
        if builtin_trait_output_is_bool(trait_id) {
            return self.bool();
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: inner_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: Vec::new(),
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(output)
    }

    fn can_expected_type_drive_builtin_operator(
        &self,
        expected: InternedTyId,
        op: BinaryOp,
    ) -> bool {
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                self.is_numeric(expected)
            }
            BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::Shl
            | BinaryOp::Shr => self.is_integer(expected),
            BinaryOp::Eq | BinaryOp::Ne => true,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => self.is_numeric(expected),
            _ => false,
        }
    }

    pub(crate) fn is_numeric_literal_expr(&self, expr: &Expr) -> bool {
        matches!(expr.kind, ExprKind::Integer(_) | ExprKind::Float(_))
            || matches!(
                &expr.kind,
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr,
                } if matches!(expr.kind, ExprKind::Integer(_) | ExprKind::Float(_))
            )
    }

    fn check_cast(&mut self, span: Span, source: InternedTyId, target: InternedTyId) {
        if source == self.error() || target == self.error() || self.is_valid_cast(source, target) {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "invalid cast: cannot cast {} to {}",
                self.ty_name(source),
                self.ty_name(target)
            ),
        ));
    }

    fn is_valid_cast(&self, source: InternedTyId, target: InternedTyId) -> bool {
        let source = self.normalization.normalize(source);
        let target = self.normalization.normalize(target);
        if source == target {
            return true;
        }
        if self.is_numeric(source) && self.is_numeric(target) {
            return true;
        }
        if self.is_char(source) && self.is_u32(target) {
            return true;
        }
        if self.is_enum(source) && self.is_integer(target) {
            return true;
        }
        if self.is_integer(source) && self.is_open_enum(target) {
            return true;
        }
        if self.is_pointer(source) && self.is_pointer(target) {
            return true;
        }
        if self.is_pointer(source) && self.is_pointer_integer(target) {
            return true;
        }
        if self.is_pointer_integer(source) && self.is_pointer(target) {
            return true;
        }
        false
    }

    pub(crate) fn check_bracket_suffix_expr(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[BracketArg],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        if args.len() == 1
            && let Some(arg) = args.first()
            && let Some(index) = &arg.expr
        {
            self.record_bracket_suffix_node_resolution(expr, BracketSuffixResolution::Index);
            if let Some(ty) = self.comptime_index_expr_runtime_type(callee, index) {
                self.check_expr(index);
                return ty;
            }
            let lhs_expected = self.array_expected_from_index_expected(expected);
            let lhs_ty = self.check_expr_with_expected(callee, lhs_expected);
            let index_ty = self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::IndexRead, index);
            self.expect_integer(index.span, index_ty, "index");
            let index_ty = self.expr_ty(index).unwrap_or(index_ty);
            if index_ty == self.error() {
                return self.error();
            }
            return self.index_result_type_for_index(span, lhs_ty, index_ty);
        }
        if args.len() > 1 {
            self.diagnostics.push(Diagnostic::error(
                span,
                "multiple bracket arguments are only valid for generic instantiation",
            ));
        } else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "generic instantiation must be used as a callee or type prefix",
            ));
        }
        self.check_expr(callee);
        for arg in args {
            if let Some(expr) = &arg.expr {
                self.check_expr(expr);
            }
        }
        self.error()
    }

    fn ident_type(&mut self, expr: &Expr) -> InternedTyId {
        let span = expr.span;
        match self.local_use(expr) {
            Some(LocalUse::Local(local_id)) => {
                self.local_types.get(&local_id).copied().unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "local used before its type is known",
                    ));
                    self.error()
                })
            }
            Some(LocalUse::ModuleValue) => {
                if let Some(enum_id) = self.values.node_variant_enums.get(&expr.node_key).copied() {
                    return self.interner.intern(TyKind::Nominal {
                        def_id: enum_id,
                        args: Vec::new(),
                    });
                }
                if self
                    .values
                    .node_qualified_values
                    .contains_key(&expr.node_key)
                {
                    return self
                        .qualified_global_type(expr)
                        .unwrap_or_else(|| self.error());
                }
                match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) => self.module_value_type(def_id, span),
                    _ => self.error(),
                }
            }
            Some(LocalUse::ImportAlias)
            | Some(LocalUse::TypePrefix)
            | Some(LocalUse::Unresolved)
            | None => {
                self.diagnostics
                    .push(Diagnostic::error(span, "name is unresolved"));
                self.error()
            }
        }
    }

    fn module_value_type(&mut self, def_id: DefId, span: Span) -> InternedTyId {
        let Some(def) = self.defs.defs.get(def_id) else {
            return self.error();
        };
        match def.kind {
            DefKind::Function | DefKind::Method => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "function values are not supported in this body-check stage",
                ));
                self.error()
            }
            DefKind::Global => self.global_types.get(&def_id).copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "global type is not available during body check",
                ));
                self.error()
            }),
            DefKind::Comptime => self
                .comptime_types
                .get(&def_id)
                .copied()
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "comptime type is not available during body check",
                    ));
                    self.error()
                }),
            _ => self.error(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ExpectedRangeParts {
    ty: InternedTyId,
    kind: RangeTyKind,
    bound: Option<InternedTyId>,
}

fn builtin_trait_output_is_bool(trait_id: BuiltinTrait) -> bool {
    matches!(
        trait_id,
        BuiltinTrait::Not | BuiltinTrait::Eq | BuiltinTrait::Ord
    )
}
