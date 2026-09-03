// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{float_literal_suffix_ty, integer_literal_suffix_ty};
use nia_ast::{AssignOp, BinaryOp, BracketArg, Expr, ExprKind, IndexArg, UnaryOp};
use nia_defs::{DefId, DefKind, VisibleExtensionAssociatedValue};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinAssociatedConst, GlobalDefId, InternedTyId};
use nia_local_resolve::LocalUse;
use nia_sema_ir::{
    AssociatedConstProjection, BracketSuffixResolution, BuiltinAssociatedValue, BuiltinOperatorOp,
    BuiltinValue, ResolvedCall,
};
use nia_span::Span;
use nia_symbol::{SymbolId, known};
use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, RangeTyKind, TraitId, TyKind};
use nia_value_resolve::ValueNameResolution;

struct BuiltinOperatorFinish<'a> {
    span: Span,
    trait_id: BuiltinTrait,
    op: BuiltinOperatorOp,
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
        if expr_allows_expected_const_projection(expr)
            && let Some(ty) = expected
                .and_then(|expected| self.expected_const_expr_runtime_projection(expr, expected))
        {
            self.record_expr_node_type(expr, ty);
            return ty;
        }
        let ty = match &expr.kind {
            ExprKind::Error | ExprKind::Raw(_) => self.error(),
            ExprKind::Integer(_) => self.integer_literal_type(expr),
            ExprKind::Float(_) => self.float_literal_type(expr),
            ExprKind::String(text) => self.string_literal_type(text),
            ExprKind::ByteString(text) => self.byte_string_literal_type(text),
            ExprKind::Char(_) => self.primitive(PrimitiveTy::Char),
            ExprKind::ByteChar(_) => self.primitive(PrimitiveTy::U8),
            ExprKind::Bool(_) => self.bool(),
            ExprKind::Null => self.check_null_expr(expr.span, expected),
            ExprKind::Underscore => self.error(),
            ExprKind::Ident(_) | ExprKind::SelfValue => self.ident_type(expr),
            ExprKind::PathRoot(_) => self.error(),
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => self.error(),
            ExprKind::BracketSuffix { callee, args } => {
                self.check_bracket_suffix_expr(expr, callee, args, expected)
            }
            ExprKind::Tuple(elems) => {
                let expected_elems = expected.and_then(|expected| {
                    match self.interner.get(self.normalization.normalize(expected)) {
                        Some(TyKind::Tuple(expected_elems))
                            if expected_elems.len() == elems.len() =>
                        {
                            Some(expected_elems.clone())
                        }
                        _ => None,
                    }
                });
                let elem_types = elems
                    .iter()
                    .enumerate()
                    .map(|(index, elem)| {
                        let expected = expected_elems.as_ref().map(|elems| elems[index]);
                        let actual = self.check_expr_with_expected(elem, expected);
                        if let Some(expected) = expected {
                            self.expect_expr_type(elem, expected, actual, "tuple element");
                        }
                        actual
                    })
                    .collect();
                self.interner.intern(TyKind::Tuple(elem_types))
            }
            ExprKind::Closure {
                captures,
                params,
                body,
            } => self.check_closure_expr(expr, captures, params, body, expected),
            ExprKind::ArrayLiteral { elems } => match self.expected_array_type(expected) {
                Some(expected) => self.check_array_literal(expr.span, Some(expected), elems),
                // An array literal carries its length structurally and its
                // elements can constrain their shared type without a binding-
                // specific entry point. Keeping this path expression-general
                // makes `[1i64, 2]`, call arguments, and borrowed temporaries
                // obey the same inference rule.
                None => self.infer_array_literal_expr(expr),
            },
            ExprKind::TypedStructLiteral { ty, fields } => {
                let explicit = self.ty_for_type(ty);
                self.check_struct_literal(expr.span, explicit, fields)
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                self.check_qualified_struct_literal(expr, target, fields, expected)
            }
            ExprKind::OmittedAggregateLiteral { fields } => {
                let Some(expected) = expected else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "omitted constructor requires an expected nominal type",
                    ));
                    return self.error();
                };
                self.check_struct_literal(expr.span, expected, fields)
            }
            ExprKind::OmittedMember { name } => {
                if let Some(expected) = expected
                    && let Some((enum_id, variant_def)) =
                        self.omitted_enum_variant_info(expr, expected)
                {
                    let variant_id = GlobalDefId {
                        module_id: enum_id.module_id,
                        def_id: variant_def,
                    };
                    if let Some((_, variant)) = self.resolved_enum_variant(variant_id)
                        && !matches!(
                            variant.payload,
                            nia_item_signatures::EnumVariantPayloadSignature::Unit
                        )
                    {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            expr.span,
                            format!(
                                "enum variant `{}` requires a payload",
                                self.symbol_name(*name)
                            ),
                        ));
                    }
                    expected
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "omitted member requires a call or enum expected type",
                    ));
                    self.error()
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                let expected_ref_target = self
                    .expected_ref_target_from_expected(*op, expected)
                    .or_else(|| {
                        (matches!(inner.kind, ExprKind::Closure { .. })
                            && expected.is_some_and(|ty| self.callable_signature(ty).is_some()))
                        .then_some(expected?)
                    });
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && let Some(function_ptr_ty) =
                        self.check_function_ref(inner, matches!(op, UnaryOp::RefReadOnly), expected)
                {
                    self.record_expr_node_type(expr, function_ptr_ty);
                    return function_ptr_ty;
                }
                if matches!(op, UnaryOp::RefReadOnly)
                    && let Some(expected) = expected
                    && let Some(inner_ty) = self.expr_ty(inner)
                {
                    match self.closure_to_function_pointer(expected, inner_ty) {
                        crate::callable_views::ClosureFunctionPointerCoercion::Compatible => {
                            self.record_expr_node_type(expr, expected);
                            return expected;
                        }
                        crate::callable_views::ClosureFunctionPointerCoercion::Capturing => {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_CHECK,
                                expr.span,
                                "capturing closures cannot be converted to thin function pointers; use `&Fn(...)`",
                            ));
                            return self.error();
                        }
                        crate::callable_views::ClosureFunctionPointerCoercion::Mismatch => {}
                    }
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
                match op {
                    UnaryOp::Neg | UnaryOp::Not | UnaryOp::BitNot => {
                        let signed_integer_literal_ty = if matches!(op, UnaryOp::Neg)
                            && matches!(inner.kind, ExprKind::Integer(_))
                        {
                            integer_literal_suffix_ty(expr)
                                .map(|primitive| self.primitive(primitive))
                        } else {
                            None
                        };
                        let expected = if matches!(op, UnaryOp::Neg) {
                            signed_integer_literal_ty
                                .or_else(|| {
                                    float_literal_suffix_ty(expr)
                                        .map(|primitive| self.primitive(primitive))
                                })
                                .or(expected_ref_target)
                        } else {
                            expected_ref_target
                        };
                        let inner_ty = if let Some(suffix_ty) = signed_integer_literal_ty {
                            self.check_integer_literal_range(expr, suffix_ty, "literal suffix");
                            self.record_expr_node_type(inner, suffix_ty);
                            suffix_ty
                        } else {
                            self.check_expr_with_expected(inner, expected)
                        };
                        if let Some(expected) = expected {
                            if signed_integer_literal_ty.is_some() {
                                self.expect_type(inner.span, expected, inner_ty, "unary operator");
                            } else {
                                self.expect_expr_type(inner, expected, inner_ty, "unary operator");
                            }
                        }
                        self.check_builtin_unary_operator_expr(expr.span, *op, inner, inner_ty)
                    }
                    UnaryOp::RefReadOnly => {
                        let inner_ty = self.check_expr_with_expected(inner, expected_ref_target);
                        let inner_ty = expected_ref_target
                            .and_then(|expected| {
                                self.materialize_inferred_array_type(expected, inner_ty)
                            })
                            .unwrap_or(inner_ty);
                        if self.is_invalid_temporary_type(inner_ty) {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_CHECK,
                                inner.span,
                                "reference target cannot have an uninhabited type",
                            ));
                        }
                        self.check_reference_target(inner, "reference target", true);
                        self.interner.intern(TyKind::Pointer {
                            is_readonly: true,
                            elem: inner_ty,
                        })
                    }
                    UnaryOp::Ref => {
                        let inner_ty = self.check_expr_with_expected(inner, expected_ref_target);
                        let inner_ty = expected_ref_target
                            .and_then(|expected| {
                                self.materialize_inferred_array_type(expected, inner_ty)
                            })
                            .unwrap_or(inner_ty);
                        if self.is_invalid_temporary_type(inner_ty) {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_CHECK,
                                inner.span,
                                "reference target cannot have an uninhabited type",
                            ));
                        }
                        self.check_reference_target(inner, "reference target", false);
                        self.interner.intern(TyKind::Pointer {
                            is_readonly: false,
                            elem: inner_ty,
                        })
                    }
                    UnaryOp::Deref => {
                        let expected = self.pointer_to_deref_expected(expected);
                        self.check_expr_with_expected(inner, expected);
                        let inner_ty = self.expr_runtime_ty(inner);
                        self.deref_result_type(expr.span, inner_ty)
                    }
                }
            }
            ExprKind::OptionalSome { expr: inner } => {
                self.check_optional_some_expr(inner, expected)
            }
            ExprKind::ErrorOk { expr: inner } => self.check_error_ok_expr(inner, expected),
            ExprKind::ErrorErr { expr: inner } => self.check_error_err_expr(inner, expected),
            ExprKind::Try { expr: inner } => self.check_try_expr(expr, inner),
            ExprKind::Binary { lhs, op, rhs } => {
                self.check_binary_expr(expr.span, lhs, *op, rhs, expected)
            }
            ExprKind::Assign { lhs, op, rhs } => {
                if matches!(lhs.kind, ExprKind::Underscore) {
                    self.check_expr(rhs);
                    if !matches!(op, AssignOp::Assign) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            expr.span,
                            "`_` discard only supports plain assignment",
                        ));
                    }
                    self.unit()
                } else {
                    self.check_assignment_lhs(lhs);
                    self.check_assignable(lhs, "assignment target");
                    let lhs_ty = self.assignable_expr_type(lhs);
                    if let Some(binary_op) = assign_op_binary(*op) {
                        self.check_compound_assignment(expr.span, lhs_ty, binary_op, rhs);
                    } else {
                        let rhs_ty = self.check_expr_with_expected(rhs, Some(lhs_ty));
                        self.expect_expr_type(rhs, lhs_ty, rhs_ty, "assignment");
                    }
                    self.unit()
                }
            }
            ExprKind::Cast { expr: inner, ty } => {
                let source = self.check_expr(inner);
                let target = self.ty_for_type(ty);
                if let Some(coerced) = self
                    .coerce_pointer_array_to_slice(inner, target, source)
                    .or_else(|| self.coerce_mutable_pointer_to_readonly(target, source))
                    .or_else(|| {
                        self.coerce_pointer_array_to_slice_trait_object(inner, target, source)
                    })
                {
                    self.record_expr_node_type(inner, coerced);
                } else {
                    self.check_cast(expr.span, source, target);
                }
                if self.is_open_enum(target) {
                    self.check_integer_literal_enum_backing_range(inner, target, "cast");
                }
                target
            }
            ExprKind::Call { callee, args } => self
                .check_tuple_struct_call(expr, callee, args)
                .or_else(|| self.check_enum_variant_call(expr, callee, args))
                .unwrap_or_else(|| self.check_call(expr, callee, args, expected)),
            ExprKind::Field { lhs, name } => self.check_field_access(expr, lhs, name),
            ExprKind::TupleField { lhs, index } => {
                let lhs_ty = self.check_expr(lhs);
                self.tuple_field_type(expr.span, lhs_ty, *index)
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(builtin) = crate::calls::std_builtin_function(expr) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("builtin `{}` must be called", builtin.name()),
                    ));
                    self.error()
                } else if let ExprKind::TraitTarget { ty, trait_ref } = &lhs.kind {
                    self.check_trait_associated_const_value_access(expr, ty, trait_ref, name)
                } else if let Some(ty) = self.check_builtin_associated_value(expr) {
                    ty
                } else if let Some(ty) = self.check_enum_variant_access(expr.span, lhs, name) {
                    ty
                } else if let Some(ty) = self.check_associated_const_value_access(expr, lhs, name) {
                    ty
                } else if self
                    .values
                    .node_qualified_values
                    .contains_key(&expr.node_key)
                {
                    self.qualified_global_type(expr)
                        .unwrap_or_else(|| self.error())
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "qualified access is not a value expression",
                    ));
                    self.error()
                }
            }
            ExprKind::Index { lhs, index } => {
                let lhs_expected = match index {
                    IndexArg::Expr(_) if self.expr_ty(lhs).is_none() => {
                        self.index_lhs_expected_from_index_expected(expected)
                    }
                    IndexArg::Expr(_) | IndexArg::Range(_) => None,
                };
                let lhs_ty = self.check_expr_with_expected(lhs, lhs_expected);
                if let IndexArg::Expr(index) = index
                    && matches!(self.interner.get(lhs_ty), Some(TyKind::ConstOnly))
                    && let Some(ty) = self.const_index_expr_runtime_type(lhs, index)
                {
                    self.check_expr(index);
                    return ty;
                }
                if matches!(index, IndexArg::Range(_))
                    && self.in_const_context()
                    && let Some(ty) = self.const_slice_expr_runtime_type(expr, expected)
                {
                    return ty;
                }
                let lhs_ty = self.expr_runtime_ty(lhs);
                match index {
                    IndexArg::Expr(index) => {
                        let index_ty =
                            self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::Index, index);
                        let index_ty = self.expr_ty(index).unwrap_or(index_ty);
                        if index_ty == self.error() {
                            return self.error();
                        }
                        self.index_result_type_for_index(expr.span, lhs_ty, index_ty)
                    }
                    IndexArg::Range(range) => {
                        self.check_slice_range_bounds(range);
                        self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                            expr.span,
                            "range index expression must be taken as a slice pointer; use `&base[..]` or `&mut base[..]`",
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
            ExprKind::IfPattern(if_pattern) => self.check_if_pattern_expr(if_pattern, expected),
            ExprKind::Match(matched) => self.check_match_expr(matched, expected),
        };
        let ty = if let Some(expected) = expected {
            self.coerce_pointer_array_to_slice(expr, expected, ty)
                .or_else(|| self.coerce_mutable_pointer_to_readonly(expected, ty))
                .or_else(|| self.coerce_trait_object_to_supertrait(expr, expected, ty))
                .or_else(|| self.coerce_pointer_array_to_slice_trait_object(expr, expected, ty))
                .or_else(|| self.coerce_pointer_to_trait_object(expr, expected, ty))
                .or_else(|| self.materialize_inferred_array_type(expected, ty))
                .unwrap_or(ty)
        } else {
            ty
        };
        self.record_expr_node_type(expr, ty);
        ty
    }

    fn check_closure_expr(
        &mut self,
        expr: &Expr,
        captures: &[nia_ast::ClosureCapture],
        params: &[nia_ast::Param],
        body: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let Some(owner) = self.current_def_id else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "closure expressions are only supported inside function bodies",
            ));
            return self.error();
        };
        let closure_id = self
            .expr_ty(expr)
            .and_then(|ty| match self.interner.get(ty) {
                Some(TyKind::ClosureState { closure_id, .. }) if closure_id.owner == owner => {
                    Some(*closure_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                let closure_id = nia_ids::ClosureId {
                    owner,
                    ordinal: self.next_closure_ordinal,
                };
                self.next_closure_ordinal = self.next_closure_ordinal.saturating_add(1);
                closure_id
            });

        let mut capture_types = Vec::with_capacity(captures.len());
        for capture in captures {
            let ty = self.check_expr(&capture.value);
            capture_types.push(ty);
            if let Some(local_id) = self.local_def(&capture.node_key) {
                self.record_local_type(local_id, ty);
            }
        }
        let inferred = self.inferred_closures.get(&expr.node_key).cloned();
        let expected_signature = expected.and_then(|expected| self.callable_signature(expected));
        if let Some((expected_params, _)) = &expected_signature
            && expected_params.len() != params.len()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "closure parameter count mismatch: expected {}, got {}",
                    expected_params.len(),
                    params.len()
                ),
            ));
        }
        let mut param_types = Vec::with_capacity(params.len());
        let mut param_locals = Vec::with_capacity(params.len());
        for (index, param) in params.iter().enumerate() {
            if param.receiver.is_some() {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    param.span,
                    "closure parameters cannot be receivers",
                ));
            }
            let explicit = param.ty.as_ref().map(|ty| self.ty_for_type(ty));
            let expected = expected_signature
                .as_ref()
                .and_then(|(params, _)| params.get(index).copied());
            let inferred = inferred
                .as_ref()
                .and_then(|signature| signature.params.get(index))
                .and_then(|ty| self.materialize_inferred_type(ty));
            let ty = explicit.or(expected).or(inferred).unwrap_or_else(|| {
                    let name = param
                        .name
                        .map(|name| format!(" `{}`", self.symbol_name(name)))
                        .unwrap_or_default();
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        param.span,
                        format!(
                            "cannot infer closure parameter{name}; add a type annotation or provide a callable context"
                        ),
                    ));
                    self.error()
                });
            if let Some(explicit) = explicit
                && let Some(expected) = expected
            {
                self.expect_type(param.span, expected, explicit, "closure parameter");
            }
            param_types.push(ty);
            if let Some(local_id) = self.local_def(&param.node_key) {
                self.record_local_type(local_id, ty);
                param_locals.push(local_id);
            }
        }
        let expected_return = expected_signature.map(|(_, return_type)| return_type);
        let inferred_return = inferred
            .as_ref()
            .and_then(|signature| self.materialize_inferred_type(&signature.return_type));
        let declared_return = expected_return.or(inferred_return);
        let previous_return = self.current_return;
        let previous_params = std::mem::replace(&mut self.current_param_locals, param_locals);
        self.current_return = declared_return.unwrap_or_else(|| self.error());
        let body_ty = self.check_expr_with_expected(body, declared_return);
        if let Some(return_type) = declared_return {
            self.expect_expr_type(body, return_type, body_ty, "closure body");
        }
        let return_type = declared_return.unwrap_or(body_ty);
        self.current_return = previous_return;
        self.current_param_locals = previous_params;
        self.interner.intern(TyKind::ClosureState {
            closure_id,
            captures: capture_types,
            params: param_types,
            return_type,
        })
    }

    fn callable_signature(
        &mut self,
        ty: InternedTyId,
    ) -> Option<(Vec<InternedTyId>, InternedTyId)> {
        let ty = self.normalize_aliases_in_type(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Callable {
                params,
                return_type,
                ..
            }) => Some((params, return_type)),
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => Some((params, return_type)),
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            }) => Some((params, return_type)),
            Some(TyKind::Pointer { elem, .. }) => self.callable_signature(elem),
            _ => None,
        }
    }

    fn check_associated_const_value_access(
        &mut self,
        expr: &Expr,
        lhs: &Expr,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let target_ty = self.associated_target_ty(lhs, None, name)?;
        let value = self.associated_const_value_for_target(target_ty, name)?;
        let def_id = value.def_id;
        self.const_types
            .get(&def_id.def_id)
            .copied()
            .or_else(|| match def_id.module_id == self.defs.module_id {
                true => self
                    .signatures
                    .consts
                    .get(&def_id.def_id)
                    .and_then(|signature| signature.explicit_type),
                false => self
                    .program_signature_scope
                    .const_eval(def_id)
                    .and_then(|signature| signature.signature.explicit_type),
            })
            .or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "associated const value requires an explicit type",
                ));
                Some(self.error())
            })
    }

    fn check_trait_associated_const_value_access(
        &mut self,
        expr: &Expr,
        target: &nia_ast::TypeRef,
        trait_ref: &nia_ast::TypeRef,
        name: &SymbolId,
    ) -> InternedTyId {
        let target_ty = self.ty_for_type(target);
        let target_ty = self.normalize_projection(target_ty);
        let trait_ty = self.ty_for_type(trait_ref);
        let Some((trait_id, trait_args, trait_const_args)) = self.trait_id_and_args(trait_ty)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                trait_ref.span,
                "associated const projection requires a trait target",
            ));
            return self.error();
        };
        self.record_associated_const_projection(
            expr,
            AssociatedConstProjection {
                self_ty: target_ty,
                trait_id,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
                name: *name,
            },
        );
        if !self.current_context_proves_trait_obligation_with_const_args(
            target_ty,
            trait_id,
            trait_args.clone(),
            trait_const_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(target_ty),
                    self.trait_ty_name(trait_id, &trait_args)
                ),
            ));
        }
        let TraitId::Source(trait_def_id) = trait_id else {
            let TraitId::Builtin(trait_id) = trait_id else {
                unreachable!("trait_id matched source or builtin");
            };
            if let Some(associated) =
                crate::symbols::builtin_associated_const_symbol(trait_id, *name)
            {
                if matches!(trait_id, BuiltinTrait::Simd)
                    && matches!(associated, BuiltinAssociatedConst::Lanes)
                    && let Some(TyKind::Vector { lanes, .. }) =
                        self.interner.get(target_ty).cloned()
                {
                    self.record_builtin_node_value(expr, BuiltinValue::Usize(u64::from(lanes)));
                }
                return self.primitive(PrimitiveTy::Usize);
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "trait has no associated const value `{}`",
                    self.symbol_name(*name)
                ),
            ));
            return self.error();
        };
        let Some(signature) = self.resolved_trait_signature(trait_def_id) else {
            return self.error();
        };
        let Some(associated_value) = signature
            .associated_values
            .iter()
            .find(|associated_value| &associated_value.name == name)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "trait has no associated const value `{}`",
                    self.symbol_name(*name)
                ),
            ));
            return self.error();
        };
        let (substitutions, const_substitutions) = self.generic_substitutions_and_consts_for_def(
            trait_def_id,
            &trait_args,
            &trait_const_args,
        );
        let ty = self.substitute_generics_and_consts_with_self(
            associated_value.ty,
            &substitutions,
            &const_substitutions,
            target_ty,
        );
        self.normalize_projection(ty)
    }

    fn associated_const_value_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<VisibleExtensionAssociatedValue> {
        let mut matches = Vec::new();
        let extension_targets =
            self.with_visible_extensions(|extensions| extensions.targets().to_vec());
        for extension_target in &extension_targets {
            let mut substitutions = nia_symbol::SymbolMap::default();
            let mut const_substitutions = nia_symbol::SymbolMap::default();
            if !self.match_type_pattern_with_consts(
                extension_target.target_ty,
                target_ty,
                &mut substitutions,
                &mut const_substitutions,
            ) {
                continue;
            }
            for value in &extension_target.associated_values {
                if &value.name == name {
                    matches.push(value.clone());
                }
            }
        }
        let first = matches.first()?.clone();
        if matches.len() > 1 {
            return None;
        }
        Some(first)
    }

    fn check_builtin_associated_value(&mut self, expr: &Expr) -> Option<InternedTyId> {
        let value = self
            .semantic_uses
            .node_builtin_associated_value(&expr.node_key)?;
        match value {
            BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } => {
                let ty = self.primitive(primitive);
                let value = kind.value(primitive, self.target.pointer_width)?;
                let builtin = if primitive == PrimitiveTy::Usize {
                    BuiltinValue::Usize(u64::try_from(value.bits()).ok()?)
                } else {
                    BuiltinValue::Int(value)
                };
                self.record_builtin_node_value(expr, builtin);
                Some(ty)
            }
        }
    }

    fn expected_const_expr_runtime_projection(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        let const_expr = self.lower_const_expr(expr).ok()?;
        match self.const_expr_type_for_ir_with_expected(&const_expr, Some(expected))? {
            nia_const_check::ConstValueType::Runtime(actual)
                if self.types_match(expected, actual) =>
            {
                Some(
                    self.materialize_inferred_array_type(expected, actual)
                        .unwrap_or(expected),
                )
            }
            nia_const_check::ConstValueType::Runtime(actual) => {
                self.coerce_pointer_array_to_slice(expr, expected, actual)
            }
            nia_const_check::ConstValueType::String
                if self.is_runtime_char_array_type(expected) =>
            {
                Some(match &expr.kind {
                    ExprKind::String(literal) => {
                        self.materialize_string_literal_expected_array(literal, expected)
                    }
                    _ => expected,
                })
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

    fn materialize_string_literal_expected_array(
        &mut self,
        literal: &nia_ast::StringLiteral,
        expected: InternedTyId,
    ) -> InternedTyId {
        match self.interner.get(expected) {
            Some(TyKind::Array {
                len: ArrayLenTy::Infer,
                elem,
            }) if matches!(
                self.interner.get(*elem),
                Some(TyKind::Primitive(PrimitiveTy::Char))
            ) =>
            {
                self.string_literal_array_type(literal)
            }
            _ => expected,
        }
    }

    fn check_null_expr(&mut self, span: Span, expected: Option<InternedTyId>) -> InternedTyId {
        let Some(expected) = expected.map(|ty| self.normalization.normalize(ty)) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "`null` requires an expected optional type",
            ));
            return self.error();
        };
        if matches!(self.interner.get(expected), Some(TyKind::Optional { .. })) {
            expected
        } else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "`null` requires an optional type, found `{}`",
                    self.ty_name(expected)
                ),
            ));
            self.error()
        }
    }

    fn expected_array_type(&self, expected: Option<InternedTyId>) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        matches!(self.interner.get(expected), Some(TyKind::Array { .. })).then_some(expected)
    }

    pub(crate) fn expected_ref_target_from_expected(
        &mut self,
        op: UnaryOp,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        match (op, self.interner.get(expected).cloned()) {
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
            ) => Some(elem),
            (UnaryOp::RefReadOnly, Some(TyKind::Slice { elem, .. }))
            | (UnaryOp::Ref, Some(TyKind::Slice { elem, .. })) => {
                Some(self.interner.intern(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem,
                }))
            }
            _ => None,
        }
    }

    fn check_optional_some_expr(
        &mut self,
        inner: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let expected = expected.map(|expected| self.normalization.normalize(expected));
        let expected_elem = expected.and_then(|expected| match self.interner.get(expected) {
            Some(TyKind::Optional { elem }) => Some(*elem),
            _ => None,
        });
        let actual = self.check_expr_with_expected(inner, expected_elem);
        let elem = if let Some(expected_elem) = expected_elem {
            self.expect_expr_type(inner, expected_elem, actual, "optional value");
            expected_elem
        } else {
            actual
        };
        self.record_expr_node_type(inner, elem);
        self.interner.intern(TyKind::Optional { elem })
    }

    fn check_error_ok_expr(
        &mut self,
        inner: &Expr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let expected = expected.map(|expected| self.normalization.normalize(expected));
        let Some((error, value)) = expected.and_then(|expected| self.error_union_parts(expected))
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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
        let expected = expected.map(|expected| self.normalization.normalize(expected));
        let Some((error, value)) = expected.and_then(|expected| self.error_union_parts(expected))
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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

    fn check_try_expr(&mut self, expr: &Expr, inner: &Expr) -> InternedTyId {
        let span = expr.span;
        let inner_ty = self.check_expr(inner);
        self.record_expr_node_type(inner, inner_ty);
        let normalized = self.normalize_aliases(inner_ty);
        match self.interner.get(normalized).cloned() {
            Some(TyKind::Optional { elem }) => {
                let current_return = self.normalize_aliases(self.current_return);
                if !matches!(
                    self.interner.get(current_return),
                    Some(TyKind::Optional { .. })
                ) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
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
                            match self.resolve_into_error_conversion(span, error, return_error) {
                                Ok(Some(conversion)) => {
                                    self.record_resolved_node_call(
                                        span,
                                        &expr.node_key,
                                        conversion,
                                    );
                                }
                                Ok(None) => {
                                    self.diagnostics.push(Diagnostic::user_error_at(
                                        codes::TYPE_CHECK,
                                        span,
                                        format!(
                                            "error propagation requires `{}` to implement `IntoError[{}]`",
                                            self.ty_name(error),
                                            self.ty_name(return_error)
                                        ),
                                    ));
                                }
                                Err(()) => {}
                            }
                        }
                    }
                    None => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            span,
                            "error propagation requires an error union function return type",
                        ));
                    }
                }
                value
            }
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
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

    fn resolve_into_error_conversion(
        &mut self,
        span: Span,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    ) -> Result<Option<ResolvedCall>, ()> {
        let mut trait_ids = self
            .program_signature_scope
            .trait_ids_with_method_named(&known::INTO_ERROR);
        if let Some(def_id) = self.defs.module_scope.types.get(&known::INTO_ERROR_TRAIT)
            && self
                .defs
                .defs
                .get(def_id)
                .is_some_and(|def| def.kind == DefKind::Trait)
        {
            trait_ids.push(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            });
        }
        trait_ids.sort_unstable();
        trait_ids.dedup();

        let mut matches = Vec::new();
        let mut ambiguous = false;
        let mut has_named_protocol = false;
        let mut has_valid_protocol = false;
        let mut has_malformed_protocol = false;
        for trait_id in trait_ids {
            if self.definition_name(trait_id) != Some(known::INTO_ERROR_TRAIT) {
                continue;
            }
            has_named_protocol = true;
            let Some(signature) = self.resolved_trait_signature(trait_id) else {
                has_malformed_protocol = true;
                continue;
            };
            if signature.generics.len() != 1 {
                has_malformed_protocol = true;
                continue;
            }
            let Some(method) = signature
                .methods
                .iter()
                .find(|method| method.name == known::INTO_ERROR)
            else {
                has_malformed_protocol = true;
                continue;
            };
            if !method.signature.generic_params.is_empty()
                || method.signature.params.len() != 1
                || method.signature.params[0].receiver != Some(nia_ids::ReceiverKind::Value)
                || method.signature.is_variadic
            {
                has_malformed_protocol = true;
                continue;
            }
            has_valid_protocol = true;
            let receiver_kind = nia_ids::ReceiverKind::Value;
            let trait_args = vec![target_ty];
            let (substitutions, const_substitutions) =
                self.generic_substitutions_and_consts_for_def(trait_id, &trait_args, &[]);
            let method_return = self.substitute_generics_and_consts_with_self(
                method.signature.return_type,
                &substitutions,
                &const_substitutions,
                source_ty,
            );
            let method_return = self.normalize_projection(method_return);
            if !self.types_match(method_return, target_ty) {
                continue;
            }
            match self.current_context_resolve_trait_obligation(
                source_ty,
                TraitId::Source(trait_id),
                trait_args.clone(),
            ) {
                nia_trait_solve::TraitResolution::User(_)
                | nia_trait_solve::TraitResolution::Assumed(_) => {
                    matches.push(ResolvedCall::TraitMethod {
                        trait_id,
                        method_id: GlobalDefId {
                            module_id: trait_id.module_id,
                            def_id: method.def_id,
                        },
                        method_name: method.name,
                        self_ty: source_ty,
                        trait_args,
                        trait_const_args: Vec::new(),
                        args: Vec::new(),
                        const_args: Vec::new(),
                        receiver_kind,
                    });
                }
                nia_trait_solve::TraitResolution::Ambiguous => ambiguous = true,
                nia_trait_solve::TraitResolution::Intrinsic(_)
                | nia_trait_solve::TraitResolution::Unsatisfied => {}
            }
        }

        if ambiguous || matches.len() > 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "ambiguous error propagation conversion from `{}` to `{}`",
                    self.ty_name(source_ty),
                    self.ty_name(target_ty)
                ),
            ));
            return Err(());
        }
        if matches.is_empty() && has_named_protocol && has_malformed_protocol && !has_valid_protocol
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "malformed `IntoError` protocol while propagating `{}` to `{}`: expected `intoError(self) Target`",
                    self.ty_name(source_ty),
                    self.ty_name(target_ty)
                ),
            ));
            return Err(());
        }
        if matches.is_empty()
            && has_valid_protocol
            && self.has_into_error_chain(source_ty, target_ty)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "error propagation does not chain `IntoError` conversions from `{}` to `{}`",
                    self.ty_name(source_ty),
                    self.ty_name(target_ty)
                ),
            ));
            return Err(());
        }
        Ok(matches.pop())
    }

    fn has_into_error_chain(&mut self, source_ty: InternedTyId, target_ty: InternedTyId) -> bool {
        let mut trait_ids = self
            .program_signature_scope
            .trait_ids_with_method_named(&known::INTO_ERROR);
        if let Some(def_id) = self.defs.module_scope.types.get(&known::INTO_ERROR_TRAIT)
            && self
                .defs
                .defs
                .get(def_id)
                .is_some_and(|def| def.kind == DefKind::Trait)
        {
            trait_ids.push(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            });
        }
        trait_ids.sort_unstable();
        trait_ids.dedup();
        trait_ids.into_iter().any(|trait_id| {
            if self.definition_name(trait_id) != Some(known::INTO_ERROR_TRAIT) {
                return false;
            }
            let trait_id = nia_ty::TraitId::Source(trait_id);
            self.program_trait_impls
                .iter()
                .filter(|implementation| implementation.trait_id == trait_id)
                .filter_map(|implementation| implementation.trait_args.first().copied())
                .any(|middle_ty| {
                    if self.types_match(middle_ty, target_ty) {
                        return false;
                    }
                    let first = self.current_context_resolve_trait_obligation(
                        source_ty,
                        trait_id,
                        vec![middle_ty],
                    );
                    if !matches!(
                        first,
                        nia_trait_solve::TraitResolution::User(_)
                            | nia_trait_solve::TraitResolution::Assumed(_)
                    ) {
                        return false;
                    }
                    matches!(
                        self.current_context_resolve_trait_obligation(
                            middle_ty,
                            trait_id,
                            vec![target_ty],
                        ),
                        nia_trait_solve::TraitResolution::User(_)
                            | nia_trait_solve::TraitResolution::Assumed(_)
                    )
                })
        })
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
        let (start_ty, end_ty) = if start_expected.is_none()
            && end_expected.is_none()
            && let (Some(start), Some(end)) = (&range.start, &range.end)
        {
            self.check_range_bounds_with_peer_expected(start, end)
        } else {
            (
                range.start.as_ref().map(|start| {
                    self.check_range_bound_with_expected(start, start_expected, "range start")
                }),
                range.end.as_ref().map(|end| {
                    self.check_range_bound_with_expected(end, end_expected, "range end")
                }),
            )
        };
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
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "inclusive range expression requires an end bound",
                ));
                return self.error();
            }
        };
        if let Some(expected) = expected
            && expected.kind != kind
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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

    fn check_range_bounds_with_peer_expected(
        &mut self,
        start: &Expr,
        end: &Expr,
    ) -> (Option<InternedTyId>, Option<InternedTyId>) {
        let start_is_untyped_literal = self.is_untyped_numeric_literal_expr(start);
        let end_is_untyped_literal = self.is_untyped_numeric_literal_expr(end);
        if start_is_untyped_literal && !end_is_untyped_literal {
            let end_ty = self.check_range_bound_with_expected(end, None, "range end");
            let start_ty = self.check_range_bound_with_expected(start, Some(end_ty), "range start");
            return (Some(start_ty), Some(end_ty));
        }
        if end_is_untyped_literal && !start_is_untyped_literal {
            let start_ty = self.check_range_bound_with_expected(start, None, "range start");
            let end_ty = self.check_range_bound_with_expected(end, Some(start_ty), "range end");
            return (Some(start_ty), Some(end_ty));
        }
        (
            Some(self.check_range_bound_with_expected(start, None, "range start")),
            Some(self.check_range_bound_with_expected(end, None, "range end")),
        )
    }

    fn check_range_bound_with_expected(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
        context: &str,
    ) -> InternedTyId {
        let actual = self.check_expr_with_expected(expr, expected);
        if let Some(expected) = expected {
            self.expect_expr_type(expr, expected, actual, context);
        }
        self.expr_ty(expr).unwrap_or(actual)
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
            return self.unit();
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
            if self.is_never(else_ty) {
                let then_ty = self.check_block(then_branch);
                return self.block_tail_materialized_type(then_branch, then_ty);
            }
            let then_ty = self.check_block_with_expected(then_branch, Some(else_ty));
            self.expect_block_tail_type(then_branch, else_ty, then_ty, "if branches");
            return if self.is_never(then_ty) {
                else_ty
            } else {
                self.block_tail_materialized_type(then_branch, then_ty)
            };
        }

        let then_ty = self.check_block(then_branch);
        if self.is_never(then_ty) {
            let else_ty = self.check_expr(else_branch);
            return if self.is_never(else_ty) {
                then_ty
            } else {
                self.expr_ty(else_branch).unwrap_or(else_ty)
            };
        }
        let else_ty = self.check_expr_with_expected(else_branch, Some(then_ty));
        self.expect_expr_type(else_branch, then_ty, else_ty, "if branches");
        then_ty
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

    pub(crate) fn expect_expr_or_block_tail_type(
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
        &mut self,
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

    pub(crate) fn index_lhs_expected_from_index_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = expected?;
        let array = self.interner.intern(TyKind::Array {
            len: ArrayLenTy::Infer,
            elem: expected,
        });
        Some(array)
    }

    pub(crate) fn pointer_to_deref_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let elem = expected?;
        Some(self.interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem,
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
            | BinaryOp::Ne => self.check_builtin_operator_expr(span, lhs, op, rhs, expected),
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
                self.check_builtin_shift_expr(span, lhs, op, rhs, expected)
            }
            _ => self.check_builtin_operator_expr(span, lhs, op, rhs, expected),
        }
    }

    fn check_compound_assignment(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        op: BinaryOp,
        rhs: &Expr,
    ) {
        let rhs_expected = self.is_numeric_literal_expr(rhs).then_some(lhs_ty);
        let rhs_actual = self.check_expr_with_expected(rhs, rhs_expected);
        if let Some(expected) = rhs_expected {
            self.expect_expr_type(rhs, expected, rhs_actual, "compound assignment operand");
        }
        let rhs_ty = self.expr_ty(rhs).unwrap_or(rhs_actual);
        let Some(trait_id) = BuiltinOperatorOp::Binary(op).trait_id() else {
            return;
        };
        let trait_args = vec![rhs_ty];
        let trait_is_satisfied = self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        );
        if !trait_is_satisfied {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(trait_id, &trait_args)
                ),
            ));
            return;
        }

        let output = self.interner.intern(TyKind::Projection {
            self_ty: lhs_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args,
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        let output = self.normalize_projection(output);
        self.expect_type(span, lhs_ty, output, "compound assignment result");
    }

    fn check_builtin_shift_expr(
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
        let lhs_expected = expected
            .filter(|expected| self.can_expected_type_drive_builtin_operator(*expected, op));
        let lhs_actual = if let Some(expected) = lhs_expected {
            self.check_expr_with_expected(lhs, Some(expected))
        } else {
            self.check_expr(lhs)
        };
        if let Some(expected) = lhs_expected {
            self.expect_expr_type(lhs, expected, lhs_actual, "binary operator");
        }
        let lhs_ty = self.expr_ty(lhs).unwrap_or(lhs_actual);
        let rhs_actual = self.check_expr(rhs);
        let rhs_ty = self.expr_ty(rhs).unwrap_or(rhs_actual);
        let rhs_is_integer = self.is_integer(rhs_ty);
        if !rhs_is_integer {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                rhs.span,
                format!(
                    "shift count must be an integer type, got {}",
                    self.ty_name(rhs_ty)
                ),
            ));
        } else if !self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(trait_id),
            vec![rhs_ty],
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(trait_id, &[rhs_ty])
                ),
            ));
        }
        if let Some(expected) = expected {
            self.expect_type(span, expected, lhs_ty, "binary operator");
        }
        self.record_builtin_operator_method(BuiltinOperatorOp::Binary(op), lhs_ty, vec![rhs_ty]);
        lhs_ty
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
        let output_is_boolean = builtin_trait_output_is_boolean(trait_id);
        let mut prechecked_rhs = None;
        if self.is_untyped_numeric_literal_expr(lhs) && !self.is_numeric_literal_expr(rhs) {
            let rhs_actual = self.check_expr(rhs);
            let rhs_ty = self.expr_ty(rhs).unwrap_or(rhs_actual);
            if self.is_numeric(rhs_ty) && self.can_expected_type_drive_builtin_operator(rhs_ty, op)
            {
                let lhs_actual = self.check_expr_with_expected(lhs, Some(rhs_ty));
                self.expect_expr_type(lhs, rhs_ty, lhs_actual, "binary operator");
                return self.finish_builtin_operator_expr(BuiltinOperatorFinish {
                    span,
                    trait_id,
                    op: BuiltinOperatorOp::Binary(op),
                    lhs,
                    lhs_actual,
                    rhs,
                    rhs_actual,
                    expected,
                });
            }
            prechecked_rhs = Some(rhs_actual);
        }

        let lhs_expected = (!output_is_boolean).then_some(()).and_then(|_| {
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
        let rhs_actual = match prechecked_rhs {
            Some(rhs_actual) => rhs_actual,
            None => self.check_expr_with_expected(rhs, rhs_expected),
        };
        if let Some(expected) = rhs_expected {
            self.expect_expr_type(rhs, expected, rhs_actual, "binary operator");
        }
        self.finish_builtin_operator_expr(BuiltinOperatorFinish {
            span,
            trait_id,
            op: BuiltinOperatorOp::Binary(op),
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
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                finish.span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(finish.trait_id, &trait_args)
                ),
            ));
        }
        self.record_builtin_operator_method(finish.op, lhs_ty, trait_args.clone());

        let output = if builtin_trait_output_is_boolean(finish.trait_id) {
            self.vector_bool_mask(lhs_ty).unwrap_or_else(|| self.bool())
        } else {
            let output = self.interner.intern(TyKind::Projection {
                self_ty: lhs_ty,
                trait_id: TraitId::Builtin(finish.trait_id),
                trait_args,
                trait_const_args: Vec::new(),
                name: known::OUTPUT,
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
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(inner_ty),
                    self.builtin_trait_ty_name(trait_id, &[])
                ),
            ));
        }
        self.record_builtin_operator_method(BuiltinOperatorOp::Unary(op), inner_ty, Vec::new());
        if builtin_trait_output_is_boolean(trait_id) {
            return self
                .vector_bool_mask(inner_ty)
                .unwrap_or_else(|| self.bool());
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: inner_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        self.normalize_projection(output)
    }

    fn record_builtin_operator_method(
        &mut self,
        op: BuiltinOperatorOp,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        let (Some(trait_id), Some(method)) = (op.trait_id(), op.method()) else {
            return;
        };
        debug_assert_eq!(method.trait_id(), trait_id);
        self.record_builtin_trait_method_ref(method, self_ty, trait_args);
    }

    pub(crate) fn can_expected_type_drive_builtin_operator(
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

    pub(crate) fn is_untyped_numeric_literal_expr(&self, expr: &Expr) -> bool {
        self.is_numeric_literal_expr(expr) && !self.numeric_literal_has_suffix(expr)
    }

    fn check_cast(&mut self, span: Span, source: InternedTyId, target: InternedTyId) {
        if source == self.error() || target == self.error() || self.is_valid_cast(source, target) {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
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
            let lhs_expected = if self.expr_ty(callee).is_none() {
                self.index_lhs_expected_from_index_expected(expected)
            } else {
                None
            };
            let lhs_ty = self.check_expr_with_expected(callee, lhs_expected);
            if matches!(self.interner.get(lhs_ty), Some(TyKind::ConstOnly))
                && let Some(ty) = self.const_index_expr_runtime_type(callee, index)
            {
                self.check_expr(index);
                return ty;
            }
            let lhs_ty = self.expr_runtime_ty(callee);
            let index_ty = self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::Index, index);
            let index_ty = self.expr_ty(index).unwrap_or(index_ty);
            if index_ty == self.error() {
                return self.error();
            }
            return self.index_result_type_for_index(span, lhs_ty, index_ty);
        }
        if args.len() > 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "multiple bracket arguments are only valid for generic instantiation",
            ));
        } else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        "local used before its type is known",
                    ));
                    self.error()
                })
            }
            Some(LocalUse::Static(global_id)) if global_id.module_id == self.defs.module_id => {
                self.module_value_type(global_id.def_id, span)
            }
            Some(LocalUse::Static(_)) => self.error(),
            Some(LocalUse::ModuleValue) => {
                if let Some(enum_id) = self.values.node_variant_enums.get(&expr.node_key).copied() {
                    return self.interner.intern(TyKind::Nominal {
                        def_id: enum_id,
                        args: Vec::new(),
                        const_args: Vec::new(),
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
            Some(LocalUse::Module)
            | Some(LocalUse::TypePrefix)
            | Some(LocalUse::Unresolved)
            | None => {
                if let Some(arg) =
                    expr_ident_name(expr).and_then(|name| self.current_const_generic_arg(name))
                {
                    return arg.ty;
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "name is unresolved",
                ));
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
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "function values are not supported in this body-check stage",
                ));
                self.error()
            }
            DefKind::Global => self.global_types.get(&def_id).copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "global type is not available during body check",
                ));
                self.error()
            }),
            DefKind::Const => self.const_types.get(&def_id).copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "const type is not available during body check",
                ));
                self.error()
            }),
            _ => self.error(),
        }
    }
}

fn expr_ident_name(expr: &Expr) -> Option<&SymbolId> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name),
        _ => None,
    }
}

fn expr_allows_expected_const_projection(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::ByteString(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Raw(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Ident(_)
            | ExprKind::TypeTarget { .. }
            | ExprKind::TraitTarget { .. }
            | ExprKind::Underscore
            | ExprKind::Error
    )
}

#[derive(Debug, Clone, Copy)]
struct ExpectedRangeParts {
    ty: InternedTyId,
    kind: RangeTyKind,
    bound: Option<InternedTyId>,
}

fn builtin_trait_output_is_boolean(trait_id: BuiltinTrait) -> bool {
    matches!(
        trait_id,
        BuiltinTrait::Not | BuiltinTrait::Eq | BuiltinTrait::Ord
    )
}

fn assign_op_binary(op: AssignOp) -> Option<BinaryOp> {
    Some(match op {
        AssignOp::Assign => return None,
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Shl => BinaryOp::Shl,
        AssignOp::Shr => BinaryOp::Shr,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Rem => BinaryOp::Rem,
        AssignOp::BitAnd => BinaryOp::BitAnd,
        AssignOp::BitXor => BinaryOp::BitXor,
        AssignOp::BitOr => BinaryOp::BitOr,
    })
}
