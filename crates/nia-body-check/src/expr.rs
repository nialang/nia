// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{float_literal_suffix_ty, integer_literal_suffix_ty};
use nia_ast::{AssignOp, BinaryOp, BracketArg, Expr, ExprKind, IndexArg, UnaryOp};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_local_resolve::LocalUse;
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind};
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_expr(&mut self, expr: &Expr) -> InternedTyId {
        self.check_expr_with_expected(expr, None)
    }

    pub(crate) fn check_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
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
            ExprKind::Underscore => self.error(),
            ExprKind::Ident(_) => self.ident_type(expr.span),
            ExprKind::Builtin { name, type_arg } => self.check_builtin(expr.span, name, type_arg),
            ExprKind::TypeTarget { .. } => self.error(),
            ExprKind::BracketSuffix { callee, args } => {
                self.check_bracket_suffix_expr(expr.span, callee, args, expected)
            }
            ExprKind::ArrayLiteral { elems } => {
                self.check_array_literal(expr.span, array_expected.or(expected), elems)
            }
            ExprKind::StructLiteral { fields } => {
                self.check_struct_literal(expr.span, expected, fields)
            }
            ExprKind::Unary { op, expr: inner } => {
                let expected_ref_target = match (op, expected.and_then(|ty| self.interner.get(ty)))
                {
                    (
                        UnaryOp::RefConst,
                        Some(TyKind::Pointer {
                            is_const: true,
                            elem,
                        }),
                    )
                    | (
                        UnaryOp::Ref,
                        Some(TyKind::Pointer {
                            is_const: false,
                            elem,
                        }),
                    ) => Some(*elem),
                    _ => None,
                };
                if matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
                    && let Some(function_ptr_ty) =
                        self.check_function_ref(inner, matches!(op, UnaryOp::RefConst))
                {
                    self.expr_types.insert(expr.span, function_ptr_ty);
                    return function_ptr_ty;
                }
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                {
                    match op {
                        UnaryOp::RefConst => {
                            let slice_ty =
                                self.check_slice_ref(expr.span, lhs, range, true, expected);
                            self.expr_types.insert(inner.span, slice_ty);
                            self.expr_types.insert(expr.span, slice_ty);
                            return slice_ty;
                        }
                        UnaryOp::Ref => {
                            let slice_ty =
                                self.check_slice_ref(expr.span, lhs, range, false, expected);
                            self.expr_types.insert(inner.span, slice_ty);
                            self.expr_types.insert(expr.span, slice_ty);
                            return slice_ty;
                        }
                        _ => {}
                    }
                }
                let inner_ty = self.check_expr_with_expected(inner, expected_ref_target);
                match op {
                    UnaryOp::Neg => inner_ty,
                    UnaryOp::Not => {
                        self.expect_type(expr.span, self.bool(), inner_ty, "logical not");
                        self.bool()
                    }
                    UnaryOp::RefConst => {
                        if self.is_invalid_temporary_type(inner_ty) {
                            self.diagnostics.push(Diagnostic::error(
                                inner.span,
                                "const reference target cannot have void or never type",
                            ));
                        }
                        self.check_addressable(inner, "const reference target");
                        self.interner.intern(TyKind::Pointer {
                            is_const: true,
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
                        self.check_assignable(inner, "reference target");
                        self.interner.intern(TyKind::Pointer {
                            is_const: false,
                            elem: inner_ty,
                        })
                    }
                    UnaryOp::Deref => self.deref_result_type(expr.span, inner_ty),
                }
            }
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
                    let lhs_ty = self.check_expr(lhs);
                    let rhs_ty = self.check_expr_with_expected(rhs, Some(lhs_ty));
                    self.check_assignable(lhs, "assignment target");
                    self.expect_expr_type(rhs, lhs_ty, rhs_ty, "assignment");
                    self.void()
                }
            }
            ExprKind::Cast { expr: inner, ty } => {
                let source = self.check_expr(inner);
                let target = self.ty_for_span(ty.span);
                self.check_cast(expr.span, source, target);
                if self.is_open_enum(target) {
                    self.check_integer_literal_enum_backing_range(inner, target, "cast");
                }
                target
            }
            ExprKind::Call { callee, args } => self.check_call(expr.span, callee, args, expected),
            ExprKind::Field { lhs, name } => self.check_field_access(expr.span, lhs, name),
            ExprKind::Qualified { lhs, name } => {
                if let Some(ty) = self.check_enum_variant_access(expr.span, lhs, name) {
                    ty
                } else if self.values.qualified_values.contains_key(&expr.span) {
                    self.qualified_global_type(expr.span)
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
                let lhs_expected = match index {
                    IndexArg::Expr(_) => self.array_expected_from_index_expected(expected),
                    IndexArg::Range(_) => None,
                };
                let lhs_ty = self.check_expr_with_expected(lhs, lhs_expected);
                match index {
                    IndexArg::Expr(index) => {
                        let index_ty = self.check_expr(index);
                        self.expect_integer(index.span, index_ty, "index");
                        self.index_result_type(expr.span, lhs_ty)
                    }
                    IndexArg::Range(range) => {
                        self.check_slice_range_bounds(range);
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            "range index expression must be borrowed as a slice; use `&const base[..]` or `&base[..]`",
                        ));
                        self.slice_result_type(lhs_ty, false)
                    }
                }
            }
            ExprKind::Block(block) => self.check_block_with_expected(block, expected),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.check_if_expr(cond, then_branch, else_branch.as_deref(), expected),
            ExprKind::Switch(switch) => self.check_switch_expr(switch, expected),
        };
        let ty = if let Some(expected) = expected {
            self.coerce_c_string_to_pointer(expr, expected, ty)
                .or_else(|| self.coerce_array_to_slice(expr, expected, ty))
                .or_else(|| self.materialize_inferred_array_type(expected, ty))
                .unwrap_or(ty)
        } else {
            ty
        };
        self.expr_types.insert(expr.span, ty);
        ty
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
            self.expr_types
                .get(&else_branch.span)
                .copied()
                .unwrap_or(else_ty)
        } else {
            then_ty
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
            .and_then(|tail| self.expr_types.get(&tail.span).copied())
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
                let (lhs_ty, _) =
                    self.check_same_type_binary_operands(lhs, rhs, "binary comparison");
                if matches!(
                    op,
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
                ) && !self.is_numeric(lhs_ty)
                {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "ordered comparison requires numeric operands",
                    ));
                }
                self.bool()
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
                let Some(expected) = expected.filter(|ty| self.is_integer(*ty)) else {
                    let (lhs_ty, _) =
                        self.check_same_type_binary_operands(lhs, rhs, "binary operator");
                    if !self.is_integer(lhs_ty) {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            "bitwise operator requires integer operands",
                        ));
                    }
                    return lhs_ty;
                };
                let lhs_ty = self.check_expr_with_expected(lhs, Some(expected));
                self.expect_expr_type(lhs, expected, lhs_ty, "binary operator");
                let rhs_ty = self.check_expr_with_expected(rhs, Some(expected));
                self.expect_expr_type(rhs, expected, rhs_ty, "binary operator");
                expected
            }
            BinaryOp::Shl | BinaryOp::Shr => {
                let lhs_expected = expected.filter(|ty| self.is_integer(*ty));
                let lhs_actual = self.check_expr_with_expected(lhs, lhs_expected);
                if let Some(expected) = lhs_expected {
                    self.expect_expr_type(lhs, expected, lhs_actual, "shift operator");
                }
                let lhs_ty = self
                    .expr_types
                    .get(&lhs.span)
                    .copied()
                    .unwrap_or(lhs_actual);
                if !self.is_integer(lhs_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "shift operator requires integer left operand",
                    ));
                }
                let rhs_ty = if self.is_numeric_literal_expr(rhs) {
                    self.check_expr_with_expected(rhs, Some(lhs_ty))
                } else {
                    self.check_expr(rhs)
                };
                if self.is_numeric_literal_expr(rhs) {
                    self.expect_expr_type(rhs, lhs_ty, rhs_ty, "shift operator");
                }
                if !self.is_integer(rhs_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "shift operator requires integer right operand",
                    ));
                }
                lhs_ty
            }
            _ => {
                let Some(expected) = expected.filter(|ty| self.is_numeric(*ty)) else {
                    let (lhs_ty, _) =
                        self.check_same_type_binary_operands(lhs, rhs, "binary operator");
                    if !self.is_numeric(lhs_ty) {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            "arithmetic operator requires numeric operands",
                        ));
                    }
                    return lhs_ty;
                };
                let lhs_ty = self.check_expr_with_expected(lhs, Some(expected));
                self.expect_expr_type(lhs, expected, lhs_ty, "binary operator");
                let rhs_ty = self.check_expr_with_expected(rhs, Some(expected));
                self.expect_expr_type(rhs, expected, rhs_ty, "binary operator");
                if !self.is_numeric(lhs_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "arithmetic operator requires numeric operands",
                    ));
                }
                expected
            }
        }
    }

    fn check_same_type_binary_operands(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        context: &str,
    ) -> (InternedTyId, InternedTyId) {
        if self.is_numeric_literal_expr(lhs) && !self.is_numeric_literal_expr(rhs) {
            let rhs_ty = self.check_expr(rhs);
            let lhs_actual = self.check_expr_with_expected(lhs, Some(rhs_ty));
            self.expect_expr_type(lhs, rhs_ty, lhs_actual, context);
            let lhs_ty = self
                .expr_types
                .get(&lhs.span)
                .copied()
                .unwrap_or(lhs_actual);
            return (lhs_ty, rhs_ty);
        }

        let lhs_ty = self.check_expr(lhs);
        let rhs_actual = self.check_expr_with_expected(rhs, Some(lhs_ty));
        self.expect_expr_type(rhs, lhs_ty, rhs_actual, context);
        let rhs_ty = self
            .expr_types
            .get(&rhs.span)
            .copied()
            .unwrap_or(rhs_actual);
        (lhs_ty, rhs_ty)
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

    fn check_bracket_suffix_expr(
        &mut self,
        span: Span,
        callee: &Expr,
        args: &[BracketArg],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        if args.len() == 1
            && let Some(arg) = args.first()
            && let Some(index) = &arg.expr
        {
            let lhs_expected = self.array_expected_from_index_expected(expected);
            let lhs_ty = self.check_expr_with_expected(callee, lhs_expected);
            let index_ty = self.check_expr(index);
            self.expect_integer(index.span, index_ty, "index");
            return self.index_result_type(span, lhs_ty);
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

    fn ident_type(&mut self, span: Span) -> InternedTyId {
        match self.locals.uses.get(&span) {
            Some(LocalUse::Local(local_id)) => {
                self.local_types.get(local_id).copied().unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "local used before its type is known",
                    ));
                    self.error()
                })
            }
            Some(LocalUse::ModuleValue) => {
                if let Some(enum_id) = self.values.variant_enums.get(&span).copied() {
                    return self.interner.intern(TyKind::Nominal {
                        def_id: enum_id,
                        args: Vec::new(),
                    });
                }
                if self.values.qualified_values.contains_key(&span) {
                    return self
                        .qualified_global_type(span)
                        .unwrap_or_else(|| self.error());
                }
                match self.values.names.get(&span) {
                    Some(ValueNameResolution::Def(def_id)) => self.module_value_type(*def_id, span),
                    _ => self.error(),
                }
            }
            Some(LocalUse::ImportAlias)
            | Some(LocalUse::TypePrefix)
            | Some(LocalUse::Unresolved)
            | None => self.error(),
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
