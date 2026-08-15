// SPDX-License-Identifier: GPL-3.0-or-later
use crate::literals::{
    byte_string_literal_len, float_literal_suffix_ty, float_literal_text,
    has_numeric_literal_suffix, integer_literal_suffix_ty, integer_literal_value, integer_range,
    numeric_literal_suffix, parse_float_literal, string_literal_char_len,
};
use crate::{BodyChecker, BodyTypeCx};
use nia_ast::{Expr, ExprKind, TypeRef, UnaryOp};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalConstExprId, InternedTyId};
use nia_sema_ir::PointerArrayToSliceCoercion;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, symbol_text_or_unresolved};
use nia_trait_solve::TraitSolverContext;
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, ConstGenericArg, ConstGenericValue, IntConst, PrimitiveTy,
    TraitId, TyKind,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectionNormalizationKey {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
    name: SymbolId,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn tuple_field_type(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        index: usize,
    ) -> InternedTyId {
        match self
            .interner
            .get(self.normalization.normalize(lhs_ty))
            .cloned()
        {
            Some(TyKind::Tuple(elems)) => elems.get(index).copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "tuple field index {index} is out of bounds for tuple of arity {}",
                        elems.len()
                    ),
                ));
                self.error()
            }),
            Some(TyKind::Nominal { def_id, .. }) => {
                let Some(resolved) = self.resolved_struct_signature(def_id) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!(
                            "cannot project tuple field .{index} from {}",
                            self.ty_name(lhs_ty)
                        ),
                    ));
                    return self.error();
                };
                if !resolved.signature.is_tuple {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!(
                            "cannot project tuple field .{index} from {}",
                            self.ty_name(lhs_ty)
                        ),
                    ));
                    return self.error();
                }
                let arity = resolved.signature.fields.len();
                let Some(field) = resolved.signature.fields.get(index) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!(
                            "tuple field index {index} is out of bounds for tuple struct of arity {arity}"
                        ),
                    ));
                    return self.error();
                };
                self.field_ty_for_aggregate_ty(lhs_ty, &field.name)
                    .unwrap_or_else(|| self.error())
            }
            Some(TyKind::Error) => self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "cannot project tuple field .{index} from {}",
                        self.ty_name(lhs_ty)
                    ),
                ));
                self.error()
            }
        }
    }

    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.symbols, symbol)
    }

    pub(crate) fn expect_ty_kind(&self, ty: InternedTyId) -> &TyKind {
        self.interner.get(ty).unwrap_or_else(|| {
            panic!(
                "Nia ICE: body-check type {:?} is missing from type store {:?}",
                ty,
                self.type_store.id()
            )
        })
    }

    pub(crate) fn is_error_ty(&self, ty: InternedTyId) -> bool {
        matches!(self.interner.get(ty), Some(TyKind::Error))
    }

    pub(crate) fn non_error_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        (!self.is_error_ty(ty)).then_some(ty)
    }

    pub(crate) fn normalize_aliases(&mut self, ty: InternedTyId) -> InternedTyId {
        self.normalization
            .normalized
            .get(&ty)
            .copied()
            .unwrap_or(ty)
    }

    pub(crate) fn optional_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.interner.get(ty) {
            Some(TyKind::Optional { elem }) => Some(*elem),
            _ => None,
        }
    }

    pub(crate) fn error_union_parts(
        &self,
        ty: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId)> {
        match self.interner.get(ty) {
            Some(TyKind::ErrorUnion { error, value }) => Some((*error, *value)),
            _ => None,
        }
    }

    pub(crate) fn associated_type_binding_keys_match(
        &mut self,
        left: &AssociatedTypeBindingTy,
        right: &AssociatedTypeBindingTy,
    ) -> bool {
        left.name == right.name
            && left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && left
                .trait_args
                .iter()
                .zip(right.trait_args.iter())
                .all(|(left, right)| self.types_match_normalized(*left, *right))
    }

    pub(crate) fn normalize_projection(&mut self, ty: InternedTyId) -> InternedTyId {
        self.normalize_projection_inner(ty, &mut HashSet::new())
    }

    fn normalize_projection_inner(
        &mut self,
        ty: InternedTyId,
        active_projections: &mut HashSet<ProjectionNormalizationKey>,
    ) -> InternedTyId {
        let ty = self.normalize_aliases(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.normalize_projection_inner(elem, active_projections))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound =
                    bound.map(|bound| self.normalize_projection_inner(bound, active_projections));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_projection_inner(param, active_projections))
                    .collect();
                let return_type = self.normalize_projection_inner(return_type, active_projections);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_projection_inner(param, active_projections))
                    .collect();
                let return_type = self.normalize_projection_inner(return_type, active_projections);
                self.interner.intern(TyKind::Callable {
                    is_readonly,
                    params,
                    return_type,
                })
            }
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_projection_inner(param, active_projections))
                    .collect();
                let return_type = self.normalize_projection_inner(return_type, active_projections);
                self.interner.intern(TyKind::CallablePointee {
                    params,
                    return_type,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_projection_inner(error, active_projections);
                let value = self.normalize_projection_inner(value, active_projections);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                self.interner.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::BuiltinType(_)) => ty,
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_projection_inner(arg.ty, active_projections);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.normalize_projection_inner(arg, active_projections))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty =
                                    self.normalize_projection_inner(arg.ty, active_projections);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_projection_inner(binding.ty, active_projections),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_projection_inner(arg.ty, active_projections);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.normalize_projection_inner(arg, active_projections))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty =
                                    self.normalize_projection_inner(arg.ty, active_projections);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_projection_inner(binding.ty, active_projections),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
                ..
            }) => {
                let self_ty = self.normalize_projection_inner(self_ty, active_projections);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_projection_inner(arg.ty, active_projections);
                        arg
                    })
                    .collect::<Vec<_>>();
                let key = ProjectionNormalizationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    name,
                };
                let projection = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    name,
                });
                if !active_projections.insert(key.clone()) {
                    return projection;
                }
                let normalized = self
                    .resolve_associated_type_projection(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &name,
                    )
                    .map(|resolved| self.normalize_projection_inner(resolved, active_projections))
                    .unwrap_or(projection);
                active_projections.remove(&key);
                normalized
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_)
                | TyKind::ClosureState { .. }
                | TyKind::SelfParam,
            )
            | None => ty,
        }
    }

    pub(crate) fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let assumptions = self.current_trait_goals();
        let associated_type_assumptions = self.current_associated_type_assumptions();
        let const_expr_values = self.const_expr_values_for_trait_solver(trait_const_args);
        let const_expr_value = |id, ty| const_expr_values.get(&(id, ty)).cloned();
        let program_signature_scope = self.program_signature_scope;
        let program_is_enum = move |def_id| program_signature_scope.has_enum(def_id);
        let visible_trait_witness_impls = self.visible_extension_trait_witness_impls();
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            trait_impl_index: self.program_trait_impl_index,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: self.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: Some(&const_expr_value),
            impl_is_visible: Some(&|module_id, impl_id| {
                module_id == self.defs.module_id
                    || visible_trait_witness_impls.contains(&(module_id, impl_id))
            }),
        };
        let mut solver = context
            .solver_with_associated_type_assumptions(&assumptions, &associated_type_assumptions);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, trait_const_args, name)
    }

    pub(crate) fn resolve_associated_const_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<nia_trait_solve::AssociatedConstResolution> {
        let assumptions = self.current_trait_goals();
        let const_expr_values = self.const_expr_values_for_trait_solver(trait_const_args);
        let const_expr_value = |id, ty| const_expr_values.get(&(id, ty)).cloned();
        let program_signature_scope = self.program_signature_scope;
        let program_is_enum = move |def_id| program_signature_scope.has_enum(def_id);
        let visible_trait_witness_impls = self.visible_extension_trait_witness_impls();
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            trait_impl_index: self.program_trait_impl_index,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: self.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: Some(&const_expr_value),
            impl_is_visible: Some(&|module_id, impl_id| {
                module_id == self.defs.module_id
                    || visible_trait_witness_impls.contains(&(module_id, impl_id))
            }),
        };
        let mut solver = context.solver(&assumptions);
        solver.resolve_associated_const(self_ty, trait_id, trait_args, trait_const_args, name)
    }

    pub(crate) fn expect_type(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if expected == self.error() || actual == self.error() || self.types_match(expected, actual)
        {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!(
                "type mismatch in {context}: expected {}, got {}",
                self.ty_name(expected),
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn expect_expr_type(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if has_numeric_literal_suffix(expr) {
            if let Some(primitive) = integer_literal_suffix_ty(expr) {
                let suffix_ty = self.primitive(primitive);
                self.check_integer_literal_range(expr, suffix_ty, "literal suffix");
                if self.types_match(expected, suffix_ty) {
                    self.materialize_literal_expr_type(expr, suffix_ty);
                    return;
                }
            }
            if let Some(primitive) = float_literal_suffix_ty(expr) {
                let suffix_ty = self.primitive(primitive);
                self.check_float_literal_target(expr, suffix_ty, "literal suffix");
                if self.types_match(expected, suffix_ty) {
                    self.materialize_literal_expr_type(expr, suffix_ty);
                    return;
                }
            }
        }
        if let Some(coerced) = self.coerce_pointer_array_to_slice(expr, expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_mutable_pointer_to_readonly(expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if matches!(
            expr.kind,
            ExprKind::Unary {
                op: UnaryOp::Ref | UnaryOp::RefReadOnly,
                ..
            }
        ) && let Some(coerced) = self.coerce_closure_pointer_to_callable(expected, actual)
        {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_trait_object_to_supertrait(expr, expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) =
            self.coerce_pointer_array_to_slice_trait_object(expr, expected, actual)
        {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_pointer_to_trait_object(expr, expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_integer_literal_range(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_float_literal_target(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        self.expect_type(expr.span, expected, actual, context);
    }

    pub(crate) fn is_const_only_ty(&self, ty: InternedTyId) -> bool {
        matches!(self.interner.get(ty), Some(TyKind::ConstOnly))
    }

    pub(crate) fn literal_array_expected_from_slice_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        match self.interner.get(expected).cloned() {
            Some(TyKind::Slice { elem, .. }) => Some(self.interner.intern(TyKind::Array {
                len: ArrayLenTy::Infer,
                elem,
            })),
            _ => None,
        }
    }

    pub(crate) fn coerce_pointer_array_to_slice(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Slice {
            is_readonly: expected_readonly,
            elem: expected_elem,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::Pointer {
            is_readonly: actual_readonly,
            elem: actual_elem,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if !expected_readonly && actual_readonly {
            return None;
        }
        let actual_elem = self.normalization.normalize(actual_elem);
        let Some(TyKind::Array {
            elem: actual_array_elem,
            ..
        }) = self.interner.get(actual_elem).cloned()
        else {
            return None;
        };
        if !self.types_match(expected_elem, actual_array_elem) {
            return None;
        }
        self.record_pointer_array_to_slice_node_coercion(
            expr,
            PointerArrayToSliceCoercion {
                pointer_ty: actual,
                array_ty: actual_elem,
                slice_ty: expected,
                is_readonly: expected_readonly,
            },
        );
        Some(expected)
    }

    pub(crate) fn pointer_array_slice_type(
        &mut self,
        actual: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId, bool)> {
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Pointer {
            is_readonly,
            elem: array_ty,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        let array_ty = self.normalization.normalize(array_ty);
        let Some(TyKind::Array { elem, .. }) = self.interner.get(array_ty).cloned() else {
            return None;
        };
        let slice_ty = self.interner.intern(TyKind::Slice { is_readonly, elem });
        Some((array_ty, slice_ty, is_readonly))
    }

    pub(crate) fn coerce_mutable_pointer_to_readonly(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        match (
            self.interner.get(expected).cloned(),
            self.interner.get(actual).cloned(),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: true,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: false,
                    elem: actual_elem,
                }),
            ) if self.types_match(expected_elem, actual_elem) => Some(expected),
            (
                Some(TyKind::VolatilePointer {
                    is_readonly: true,
                    elem: expected_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: false,
                    elem: actual_elem,
                }),
            ) if self.types_match(expected_elem, actual_elem) => Some(expected),
            (
                Some(TyKind::Slice {
                    is_readonly: true,
                    elem: expected_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: false,
                    elem: actual_elem,
                }),
            ) if self.types_match(expected_elem, actual_elem) => Some(expected),
            _ => None,
        }
    }

    pub(crate) fn materialize_literal_expr_type(&mut self, expr: &Expr, ty: InternedTyId) {
        self.record_expr_node_type(expr, ty);
        if let ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } = &expr.kind
        {
            self.record_expr_node_type(inner, ty);
        }
    }

    pub(crate) fn check_integer_literal_range(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} in {context}",
                    self.ty_name(expected)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_integer_literal_enum_backing_range(
        &mut self,
        expr: &Expr,
        expected_enum: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let Some(enum_id) = self.enum_global_def_id(expected_enum) else {
            return false;
        };
        let Some(signature) = self
            .resolved_enum_signature(enum_id)
            .map(|resolved| resolved.signature)
        else {
            return false;
        };
        let backing_type = signature.backing_type;
        let backing_type = self.normalization.normalize(backing_type);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(backing_type) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} backing type in {context}",
                    self.ty_name(expected_enum)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_float_literal_target(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(text) = float_literal_text(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        match primitive {
            PrimitiveTy::F32 => {
                if !parse_float_literal::<f32>(text) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("float literal `{text}` is out of range for F32 in {context}"),
                    ));
                }
                true
            }
            PrimitiveTy::F64 => {
                if !parse_float_literal::<f64>(text) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("float literal `{text}` is out of range for F64 in {context}"),
                    ));
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn numeric_literal_has_suffix(&self, expr: &Expr) -> bool {
        has_numeric_literal_suffix(expr)
    }

    pub(crate) fn report_invalid_numeric_literal_suffix(&mut self, expr: &Expr, kind: &str) {
        let suffix = numeric_literal_suffix_for_expr(expr).unwrap_or("<missing suffix>");
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            expr.span,
            format!("invalid {kind} literal suffix `{suffix}`"),
        ));
    }

    pub(crate) fn expect_integer(&mut self, span: Span, actual: InternedTyId, context: &str) {
        if actual == self.error() || self.is_integer(actual) {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!(
                "type mismatch in {context}: expected integer, got {}",
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn types_match(&mut self, expected: InternedTyId, actual: InternedTyId) -> bool {
        if let Some(matches) = self.type_match_cache.get(&(expected, actual)).copied() {
            return matches;
        }
        let matches = self.types_match_normalized(expected, actual);
        if matches || !self.type_match_depends_on_const_expr_len(expected, actual) {
            self.type_match_cache.insert((expected, actual), matches);
        }
        matches
    }

    fn type_match_depends_on_const_expr_len(
        &self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        self.type_contains_const_expr_len(expected, &mut HashSet::new())
            || self.type_contains_const_expr_len(actual, &mut HashSet::new())
    }

    fn type_contains_const_expr_len(
        &self,
        ty: InternedTyId,
        visited: &mut HashSet<InternedTyId>,
    ) -> bool {
        if !visited.insert(ty) {
            return false;
        }
        match self.interner.get(ty) {
            Some(TyKind::Array { len, elem }) => {
                matches!(len, ArrayLenTy::ConstExpr(_))
                    || self.type_contains_const_expr_len(*elem, visited)
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.type_contains_const_expr_len(*elem, visited)
            }
            Some(TyKind::Optional { elem }) => self.type_contains_const_expr_len(*elem, visited),
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.type_contains_const_expr_len(*elem, visited)),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_const_expr_len(*error, visited)
                    || self.type_contains_const_expr_len(*value, visited)
            }
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_const_expr_len(bound, visited))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                params
                    .iter()
                    .any(|param| self.type_contains_const_expr_len(*param, visited))
                    || self.type_contains_const_expr_len(*return_type, visited)
            }
            Some(TyKind::Nominal { args, .. }) => args
                .iter()
                .any(|arg| self.type_contains_const_expr_len(*arg, visited)),
            Some(TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.type_contains_const_expr_len(*arg, visited)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.type_contains_const_expr_len(*arg, visited))
                    || associated_type_bindings
                        .iter()
                        .any(|binding| self.type_contains_const_expr_len(binding.ty, visited))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.type_contains_const_expr_len(*self_ty, visited)
                    || trait_args
                        .iter()
                        .any(|arg| self.type_contains_const_expr_len(*arg, visited))
            }
            Some(
                TyKind::Primitive(_)
                | TyKind::Opaque
                | TyKind::ConstOnly
                | TyKind::Vector { .. }
                | TyKind::BuiltinType(_)
                | TyKind::GenericParam(_)
                | TyKind::ClosureState { .. }
                | TyKind::SelfParam
                | TyKind::Error,
            )
            | None => false,
        }
    }

    fn types_match_normalized(&mut self, expected: InternedTyId, actual: InternedTyId) -> bool {
        let expected = self.normalize_aliases_in_type(expected);
        let actual = self.normalize_aliases_in_type(actual);
        let expected = self.normalize_projection(expected);
        let actual = self.normalize_projection(actual);
        if self.is_never(actual) {
            return true;
        }
        if expected == actual {
            return true;
        }
        match (
            self.interner.get(expected).cloned(),
            self.interner.get(actual).cloned(),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: expected_const,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: expected_const,
                    elem: expected_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: expected_const,
                    elem: expected_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }),
            ) if expected_const == actual_const => {
                self.types_match_normalized(expected_elem, actual_elem)
            }
            (
                Some(TyKind::SlicePointee {
                    elem: expected_elem,
                }),
                Some(TyKind::SlicePointee { elem: actual_elem }),
            ) => self.types_match_normalized(expected_elem, actual_elem),
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match_normalized(expected_elem, actual_elem) => true,
            (
                Some(TyKind::Array {
                    len: expected_len,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) if self.types_match_normalized(expected_elem, actual_elem) => {
                // Type equality is used as a predicate by many callers. Array
                // length conversion failures should make the predicate false;
                // the context that produced the malformed length owns the
                // user-facing diagnostic.
                let Some(expected_len) = self.array_len_value_for_match(&expected_len) else {
                    return false;
                };
                let Some(actual_len) = self.array_len_value_for_match(&actual_len) else {
                    return false;
                };
                expected_len == actual_len
            }
            (
                Some(TyKind::Nominal {
                    def_id: expected_def,
                    args: expected_args,
                    const_args: expected_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }),
            ) => {
                expected_def == actual_def
                    && expected_args.len() == actual_args.len()
                    && expected_const_args.len() == actual_const_args.len()
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && expected_const_args
                        .iter()
                        .zip(actual_const_args.iter())
                        .all(|(expected, actual)| self.const_generic_args_match(expected, actual))
            }
            (
                Some(TyKind::Range {
                    kind: expected_kind,
                    bound: expected_bound,
                }),
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }),
            ) if expected_kind == actual_kind => match (expected_bound, actual_bound) {
                (Some(expected_bound), Some(actual_bound)) => {
                    self.types_match_normalized(expected_bound, actual_bound)
                }
                (None, None) => true,
                _ => false,
            },
            (
                Some(TyKind::Projection {
                    self_ty: expected_self,
                    trait_id: expected_trait,
                    trait_args: expected_args,
                    trait_const_args: expected_const_args,
                    name: expected_name,
                    ..
                }),
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                    ..
                }),
            ) => {
                expected_trait == actual_trait
                    && expected_name == actual_name
                    && expected_args.len() == actual_args.len()
                    && expected_const_args.len() == actual_const_args.len()
                    && self.types_match_normalized(expected_self, actual_self)
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && expected_const_args
                        .iter()
                        .zip(actual_const_args.iter())
                        .all(|(expected, actual)| self.const_generic_args_match(expected, actual))
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: expected_const,
                    trait_id: expected_trait,
                    trait_args: expected_args,
                    trait_const_args: expected_const_args,
                    associated_type_bindings: expected_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: actual_const,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) => {
                expected_const == actual_const
                    && expected_trait == actual_trait
                    && expected_args.len() == actual_args.len()
                    && expected_const_args.len() == actual_const_args.len()
                    && expected_bindings.len() == actual_bindings.len()
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && expected_const_args
                        .iter()
                        .zip(actual_const_args.iter())
                        .all(|(expected, actual)| self.const_generic_args_match(expected, actual))
                    && expected_bindings.iter().all(|expected_binding| {
                        actual_bindings
                            .iter()
                            .find(|actual_binding| {
                                actual_binding.name == expected_binding.name
                                    && actual_binding.trait_id == expected_binding.trait_id
                                    && actual_binding.trait_args.len()
                                        == expected_binding.trait_args.len()
                                    && actual_binding.trait_const_args.len()
                                        == expected_binding.trait_const_args.len()
                                    && actual_binding
                                        .trait_args
                                        .iter()
                                        .zip(expected_binding.trait_args.iter())
                                        .all(|(actual, expected)| {
                                            self.types_match_normalized(*expected, *actual)
                                        })
                                    && actual_binding
                                        .trait_const_args
                                        .iter()
                                        .zip(expected_binding.trait_const_args.iter())
                                        .all(|(actual, expected)| {
                                            self.const_generic_args_match(expected, actual)
                                        })
                            })
                            .is_some_and(|actual_binding| {
                                self.types_match_normalized(expected_binding.ty, actual_binding.ty)
                            })
                    })
            }
            (
                Some(TyKind::FunctionPointer {
                    params: expected_params,
                    return_type: expected_return,
                    is_variadic: expected_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }),
            ) => {
                expected_variadic == actual_variadic
                    && expected_params.len() == actual_params.len()
                    && expected_params
                        .iter()
                        .zip(actual_params.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && self.types_match_normalized(expected_return, actual_return)
            }
            (
                Some(TyKind::Callable {
                    is_readonly: expected_readonly,
                    params: expected_params,
                    return_type: expected_return,
                }),
                Some(TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) => {
                expected_readonly == actual_readonly
                    && expected_params.len() == actual_params.len()
                    && expected_params
                        .iter()
                        .zip(actual_params.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && self.types_match_normalized(expected_return, actual_return)
            }
            (
                Some(TyKind::CallablePointee {
                    params: expected_params,
                    return_type: expected_return,
                }),
                Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) => {
                expected_params.len() == actual_params.len()
                    && expected_params
                        .iter()
                        .zip(actual_params.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && self.types_match_normalized(expected_return, actual_return)
            }
            _ => false,
        }
    }

    pub(crate) fn const_generic_args_match(
        &mut self,
        expected: &ConstGenericArg,
        actual: &ConstGenericArg,
    ) -> bool {
        self.types_match(expected.ty, actual.ty)
            && self.const_generic_values_match(expected.ty, &expected.value, &actual.value)
    }

    pub(crate) fn const_generic_arg_slices_match(
        &mut self,
        expected: &[ConstGenericArg],
        actual: &[ConstGenericArg],
    ) -> bool {
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(expected, actual)| self.const_generic_args_match(expected, actual))
    }

    fn const_generic_values_match(
        &mut self,
        ty: InternedTyId,
        expected: &ConstGenericValue,
        actual: &ConstGenericValue,
    ) -> bool {
        if expected == actual {
            return true;
        }
        match (
            self.resolve_const_generic_value(ty, expected),
            self.resolve_const_generic_value(ty, actual),
        ) {
            (Some(ConstGenericValue::Int(left)), Some(ConstGenericValue::Int(right))) => {
                left.bits() == right.bits()
            }
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }

    fn resolve_const_generic_value(
        &mut self,
        ty: InternedTyId,
        value: &ConstGenericValue,
    ) -> Option<ConstGenericValue> {
        match value {
            ConstGenericValue::ConstExpr(id) => self.const_expr_value_for_trait_solver(*id, ty),
            ConstGenericValue::GenericParam(_) => None,
            ConstGenericValue::Int(_) | ConstGenericValue::Bool(_) | ConstGenericValue::Char(_) => {
                Some(value.clone())
            }
        }
    }

    pub(crate) fn clone_for_type_compare(&self) -> BodyChecker<'a> {
        BodyChecker {
            type_store: self.type_store,
            active_item_tree: self.active_item_tree,
            defs: self.defs,
            program: self.program,
            values: self.values,
            locals: self.locals,
            semantic_uses: self.semantic_uses,
            interner: BodyTypeCx::new(self.type_store, self.defs.module_id),
            type_lowering: self.type_lowering,
            signatures: self.signatures,
            const_signatures: self.const_signatures,
            normalization: self.normalization,
            target: self.target,
            const_eval: self.const_eval,
            const_module: self.const_module,
            layouts: self.layouts,
            extensions: self.extensions.clone(),
            program_extension_methods: self.program_extension_methods,
            program_signature_scope: self.program_signature_scope,
            program_trait_impls: self.program_trait_impls,
            program_trait_impl_index: self.program_trait_impl_index,
            program_const_values: self.program_const_values,
            program_const_array_lengths: self.program_const_array_lengths,
            program_const_module: self.program_const_module,
            source_path: self.source_path,
            symbols: self.symbols,
            extension_methods_by_id: self.extension_methods_by_id.clone(),
            extension_method_lookup_cache: self.extension_method_lookup_cache.clone(),
            callable_extension_methods_by_name: SymbolMap::default(),
            provider_demands: self.provider_demands.clone(),
            provider_demands_by_function: self.provider_demands_by_function.clone(),
            node_expr_types: HashMap::new(),
            node_bracket_suffix_resolutions: HashMap::new(),
            node_pointer_array_to_slice_coercions: HashMap::new(),
            node_trait_object_coercions: HashMap::new(),
            node_trait_object_upcasts: HashMap::new(),
            node_builtin_values: HashMap::new(),
            node_associated_const_projections: HashMap::new(),
            node_array_repeat_counts: HashMap::new(),
            node_pattern_values: HashMap::new(),
            node_resolved_calls: HashMap::new(),
            node_function_references: HashMap::new(),
            inferred_closures: self.inferred_closures.clone(),
            generic_instantiations: Vec::new(),
            function_facts: HashMap::new(),
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
            static_init_refs: HashMap::new(),
            local_types: HashMap::new(),
            global_types: HashMap::new(),
            const_types: HashMap::new(),
            method_receiver_kinds: HashMap::new(),
            traits_by_method_name: SymbolMap::default(),
            trait_impls_by_trait: HashMap::new(),
            def_trait_obligations_cache: HashMap::new(),
            trait_obligation_resolution_cache: HashMap::new(),
            type_match_cache: HashMap::new(),
            diagnostics: Vec::new(),
            diagnostic_owners: Vec::new(),
            timing: self.timing,
            timing_module_id: self.timing_module_id,
            current_return: self.current_return,
            current_def_id: self.current_def_id,
            next_closure_ordinal: self.next_closure_ordinal,
            current_param_locals: self.current_param_locals.clone(),
            const_context_depth: self.const_context_depth,
            const_call_locals: Vec::new(),
            const_eval_budget: nia_const_eval::ConstEvalBudget::default(),
            body_filter: self.body_filter.clone(),
            product: self.product,
            checked_functions: self.checked_functions.clone(),
            pending_functions: self.pending_functions.clone(),
            profile: nia_timing::TimingAccumulator::default(),
        }
    }

    pub(crate) fn materialize_inferred_array_type(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        self.materialize_inferred_array_type_inner(expected, actual)
    }

    fn materialize_inferred_array_type_inner(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        match (
            self.interner.get(expected).cloned(),
            self.interner.get(actual).cloned(),
        ) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match(expected_elem, actual_elem) => Some(actual),
            (
                Some(TyKind::Pointer { is_readonly, elem }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) if is_readonly == actual_readonly => {
                let elem = self.materialize_inferred_array_type_inner(elem, actual_elem)?;
                Some(self.interner.intern(TyKind::Pointer { is_readonly, elem }))
            }
            (
                Some(TyKind::VolatilePointer { is_readonly, elem }),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) if is_readonly == actual_readonly => {
                let elem = self.materialize_inferred_array_type_inner(elem, actual_elem)?;
                Some(
                    self.interner
                        .intern(TyKind::VolatilePointer { is_readonly, elem }),
                )
            }
            (
                Some(TyKind::Slice { is_readonly, elem }),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) if is_readonly == actual_readonly => {
                let elem = self.materialize_inferred_array_type_inner(elem, actual_elem)?;
                Some(self.interner.intern(TyKind::Slice { is_readonly, elem }))
            }
            (
                Some(TyKind::SlicePointee { elem }),
                Some(TyKind::SlicePointee { elem: actual_elem }),
            ) => {
                let elem = self.materialize_inferred_array_type_inner(elem, actual_elem)?;
                Some(self.interner.intern(TyKind::SlicePointee { elem }))
            }
            (
                Some(TyKind::Range { kind, bound }),
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }),
            ) if kind == actual_kind => match (bound, actual_bound) {
                (Some(bound), Some(actual_bound)) => {
                    let bound = self.materialize_inferred_array_type_inner(bound, actual_bound)?;
                    Some(self.interner.intern(TyKind::Range {
                        kind,
                        bound: Some(bound),
                    }))
                }
                (None, None) => Some(expected),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn def_id_for_node(
        &mut self,
        node_key: &nia_node_id::VersionedNodeKey,
        _span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let def_id = self.defs.def_nodes.get(node_key)?;
        let def = self.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn ty_for_type(&mut self, ty: &TypeRef) -> InternedTyId {
        self.type_lowering
            .ty_for_key(&ty.node_key)
            .unwrap_or_else(|| self.error())
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.normalization.normalize(ty);
        self.layouts
            .types
            .get(&ty)
            .cloned()
            .or_else(|| self.nominal_layout_of(ty))
    }

    fn nominal_layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let kind = self.interner.get(ty)?;
        let TyKind::Nominal { def_id, args, .. } = kind else {
            return None;
        };
        if def_id.module_id == self.defs.module_id {
            return self.layouts.nominal_type_layout(*def_id, args);
        }
        let layouts = (self.program.layouts?)(def_id.module_id)?;
        layouts.nominal_type_layout(*def_id, args)
    }

    pub(crate) fn array_len_value(&self, span: Span, len: &ArrayLenTy) -> Result<u64, String> {
        match len {
            ArrayLenTy::ConstValue(value) => Ok(*value),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .ok_or_else(|| "array length was not evaluated by const".to_string()),
            ArrayLenTy::Builtin { builtin, ty } => {
                let Some(layout) = self.layout_of(*ty) else {
                    return Err(format!(
                        "cannot compute layout for array length builtin `@{}`",
                        builtin.name()
                    ));
                };
                Ok(layout.builtin_value(*builtin))
            }
            ArrayLenTy::Infer => Err(format!("array length at {span:?} is not concrete")),
            ArrayLenTy::GenericParam(name) => Err(format!(
                "array length const generic `{}` at {span:?} is not substituted",
                self.symbol_name(*name)
            )),
        }
    }

    fn array_len_value_for_match(&mut self, len: &ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(*value),
            ArrayLenTy::ConstExpr(id) => {
                if let Some(value) = self.array_len_const_expr_value(*id) {
                    return Some(value);
                }
                let usize_ty = self.primitive(PrimitiveTy::Usize);
                match self.const_expr_value_for_trait_solver(*id, usize_ty)? {
                    ConstGenericValue::Int(value) => u64::try_from(value.bits()).ok(),
                    ConstGenericValue::Bool(_)
                    | ConstGenericValue::Char(_)
                    | ConstGenericValue::GenericParam(_)
                    | ConstGenericValue::ConstExpr(_) => None,
                }
            }
            ArrayLenTy::Builtin { .. } => self.array_len_value(Span::default(), len).ok(),
            ArrayLenTy::Infer | ArrayLenTy::GenericParam(_) => None,
        }
    }

    pub(crate) fn ty_name(&self, ty: InternedTyId) -> String {
        match self.interner.get(ty) {
            Some(TyKind::Opaque) => "opaque".to_string(),
            Some(TyKind::Tuple(elems)) if elems.is_empty() => "()".to_string(),
            Some(TyKind::Tuple(elems)) => {
                let names = elems
                    .iter()
                    .map(|elem| self.ty_name(*elem))
                    .collect::<Vec<_>>();
                if names.len() == 1 {
                    format!("({},)", names[0])
                } else {
                    format!("({})", names.join(", "))
                }
            }
            Some(TyKind::Primitive(primitive)) => primitive.name().to_string(),
            Some(TyKind::Vector { elem, lanes }) => format!("{}x{lanes}", elem.name()),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("&{mut_part}{}", self.ty_name(*elem))
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("^{mut_part}{}", self.ty_name(*elem))
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("&{mut_part}[{}]", self.ty_name(*elem))
            }
            Some(TyKind::SlicePointee { elem }) => format!("[{}]", self.ty_name(*elem)),
            Some(TyKind::Array { len, elem }) => {
                format!("[{}; {}]", self.ty_name(*elem), self.array_len_name(len))
            }
            Some(TyKind::Range { kind, bound }) => self.range_ty_name(*kind, *bound),
            Some(TyKind::Optional { elem }) => format!("?{}", self.ty_name(*elem)),
            Some(TyKind::ErrorUnion { error, value }) => {
                format!("{}!{}", self.ty_name(*error), self.ty_name(*value))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let mut params = params
                    .iter()
                    .map(|param| self.ty_name(*param))
                    .collect::<Vec<_>>();
                if *is_variadic {
                    params.push("...".to_string());
                }
                let return_part = if self.is_unit(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                format!("&fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }) => {
                let params = params
                    .iter()
                    .map(|param| self.ty_name(*param))
                    .collect::<Vec<_>>();
                let return_part = if self.is_unit(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("&{mut_part}Fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                let params = params
                    .iter()
                    .map(|param| self.ty_name(*param))
                    .collect::<Vec<_>>();
                let return_part = if self.is_unit(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                format!("Fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::Nominal { def_id, args, .. }) => self.nominal_ty_name(*def_id, args),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                self.builtin_trait_ty_name(*trait_id, args)
            }
            Some(TyKind::BuiltinType(builtin)) => builtin.name().to_string(),
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
                ..
            }) => self.trait_object_ty_name(
                *is_readonly,
                *trait_id,
                trait_args,
                associated_type_bindings,
            ),
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
                ..
            }) => self.trait_pointee_ty_name(*trait_id, trait_args, associated_type_bindings),
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
                ..
            }) => {
                let self_ty = self.ty_name(*self_ty);
                let trait_name = self.trait_ty_name(*trait_id, trait_args);
                let name = self.symbol_name(*name);
                format!("[{self_ty} as {trait_name}]::{name}")
            }
            Some(TyKind::GenericParam(name)) => self.symbol_name(*name),
            Some(TyKind::SelfParam) => "Self".to_string(),
            Some(TyKind::ConstOnly) => "<const-only value>".to_string(),
            Some(TyKind::Error) => "<error type>".to_string(),
            Some(TyKind::ClosureState { closure_id, .. }) => format!(
                "<closure #{}:{}>",
                closure_id.owner.def_id.0, closure_id.ordinal
            ),
            None => "<unknown type>".to_string(),
        }
    }

    fn range_ty_name(&self, kind: nia_ty::RangeTyKind, bound: Option<InternedTyId>) -> String {
        match (kind, bound) {
            (nia_ty::RangeTyKind::Exclusive, Some(bound)) => {
                format!("{}..{}", self.ty_name(bound), self.ty_name(bound))
            }
            (nia_ty::RangeTyKind::Inclusive, Some(bound)) => {
                format!("{}..={}", self.ty_name(bound), self.ty_name(bound))
            }
            (nia_ty::RangeTyKind::From, Some(bound)) => format!("{}..", self.ty_name(bound)),
            (nia_ty::RangeTyKind::To, Some(bound)) => format!("..{}", self.ty_name(bound)),
            (nia_ty::RangeTyKind::ToInclusive, Some(bound)) => {
                format!("..={}", self.ty_name(bound))
            }
            (nia_ty::RangeTyKind::Full, None) => "..".to_string(),
            _ => "<invalid range type>".to_string(),
        }
    }

    fn array_len_name(&self, len: &ArrayLenTy) -> String {
        match len {
            ArrayLenTy::Infer => "_".to_string(),
            ArrayLenTy::GenericParam(name) => self.symbol_name(*name),
            ArrayLenTy::ConstValue(value) => value.to_string(),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<unevaluated const value>".to_string()),
            ArrayLenTy::Builtin { builtin, ty } => {
                format!("@{}[{}]()", builtin.name(), self.ty_name(*ty))
            }
        }
    }

    pub(crate) fn array_len_const_expr_value(&self, id: GlobalConstExprId) -> Option<u64> {
        if id.module_id == self.defs.module_id {
            return self.const_eval.array_lengths.get(&id).copied();
        }
        (self.program_const_array_lengths)(id.module_id)
            .and_then(|array_lengths| array_lengths.values.get(&id).copied())
    }

    pub(crate) fn const_expr_values_for_trait_solver(
        &mut self,
        args: &[ConstGenericArg],
    ) -> HashMap<(GlobalConstExprId, InternedTyId), ConstGenericValue> {
        let mut values = HashMap::new();
        for arg in args {
            self.collect_const_expr_values_for_trait_solver(arg, &mut values);
        }
        values
    }

    pub(crate) fn collect_const_expr_values_for_trait_solver(
        &mut self,
        arg: &ConstGenericArg,
        values: &mut HashMap<(GlobalConstExprId, InternedTyId), ConstGenericValue>,
    ) {
        if let ConstGenericValue::ConstExpr(id) = arg.value
            && let Some(value) = self.const_expr_value_for_trait_solver(id, arg.ty)
        {
            values.insert((id, arg.ty), value);
        }
    }

    pub(crate) fn const_expr_value_for_trait_solver(
        &mut self,
        id: GlobalConstExprId,
        ty: InternedTyId,
    ) -> Option<ConstGenericValue> {
        if let Some(value) = self.array_len_const_expr_value(id) {
            return Some(ConstGenericValue::Int(IntConst::unsigned(value.into())));
        }
        if id.module_id != self.defs.module_id {
            return None;
        }
        let expr = self.type_lowering.const_exprs.get(&id)?;
        let mut checker = self.clone_for_type_compare();
        checker.eval_const_generic_expr(expr, ty)
    }

    pub(crate) fn nominal_ty_name(
        &self,
        def_id: nia_ids::GlobalDefId,
        args: &[InternedTyId],
    ) -> String {
        let base = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| {
                defs.as_ref()
                    .defs
                    .get(def_id.def_id)
                    .map(|def| self.symbol_name(def.name))
            })
            .unwrap_or_else(|| "<unknown type>".to_string());
        if args.is_empty() {
            base
        } else {
            let args = args
                .iter()
                .map(|arg| self.ty_name(*arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}[{args}]")
        }
    }

    pub(crate) fn trait_ty_name(&self, trait_id: TraitId, args: &[InternedTyId]) -> String {
        match trait_id {
            TraitId::Source(def_id) => self.nominal_ty_name(def_id, args),
            TraitId::Builtin(trait_id) => self.builtin_trait_ty_name(trait_id, args),
        }
    }

    fn trait_object_ty_name(
        &self,
        is_readonly: bool,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        associated_type_bindings: &[nia_ty::AssociatedTypeBindingTy],
    ) -> String {
        let const_part = if is_readonly { "" } else { "mut " };
        let base = match trait_id {
            TraitId::Source(def_id) => self
                .defs_for_module(def_id.module_id)
                .and_then(|defs| {
                    defs.as_ref()
                        .defs
                        .get(def_id.def_id)
                        .map(|def| self.symbol_name(def.name))
                })
                .unwrap_or_else(|| "<unknown trait>".to_string()),
            TraitId::Builtin(trait_id) => trait_id.name().to_string(),
        };
        let mut args = trait_args
            .iter()
            .map(|arg| self.ty_name(*arg))
            .collect::<Vec<_>>();
        args.extend(associated_type_bindings.iter().map(|binding| {
            let name = if let Some(trait_id) = binding.trait_id {
                format!(
                    "[Self as {}]::{}",
                    self.trait_binding_name(trait_id, &binding.trait_args),
                    self.symbol_name(binding.name)
                )
            } else {
                self.symbol_name(binding.name)
            };
            format!("{name} = {}", self.ty_name(binding.ty))
        }));
        if args.is_empty() {
            format!("&{const_part}{base}")
        } else {
            format!("&{const_part}{base}[{}]", args.join(", "))
        }
    }

    fn trait_pointee_ty_name(
        &self,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        associated_type_bindings: &[nia_ty::AssociatedTypeBindingTy],
    ) -> String {
        let base = match trait_id {
            TraitId::Source(def_id) => self
                .defs_for_module(def_id.module_id)
                .and_then(|defs| {
                    defs.as_ref()
                        .defs
                        .get(def_id.def_id)
                        .map(|def| self.symbol_name(def.name))
                })
                .unwrap_or_else(|| "<unknown trait>".to_string()),
            TraitId::Builtin(trait_id) => trait_id.name().to_string(),
        };
        let mut args = trait_args
            .iter()
            .map(|arg| self.ty_name(*arg))
            .collect::<Vec<_>>();
        args.extend(associated_type_bindings.iter().map(|binding| {
            let name = if let Some(trait_id) = binding.trait_id {
                format!(
                    "[Self as {}]::{}",
                    self.trait_binding_name(trait_id, &binding.trait_args),
                    self.symbol_name(binding.name)
                )
            } else {
                self.symbol_name(binding.name)
            };
            format!("{name} = {}", self.ty_name(binding.ty))
        }));
        if args.is_empty() {
            base
        } else {
            format!("{base}[{}]", args.join(", "))
        }
    }

    fn trait_binding_name(&self, trait_id: TraitId, trait_args: &[InternedTyId]) -> String {
        let base = match trait_id {
            TraitId::Source(def_id) => self
                .defs_for_module(def_id.module_id)
                .and_then(|defs| {
                    defs.as_ref()
                        .defs
                        .get(def_id.def_id)
                        .map(|def| self.symbol_name(def.name))
                })
                .unwrap_or_else(|| "<unknown trait>".to_string()),
            TraitId::Builtin(trait_id) => trait_id.name().to_string(),
        };
        if trait_args.is_empty() {
            base
        } else {
            format!(
                "{}[{}]",
                base,
                trait_args
                    .iter()
                    .map(|arg| self.ty_name(*arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    pub(crate) fn builtin_trait_ty_name(
        &self,
        trait_id: nia_ty::BuiltinTrait,
        args: &[InternedTyId],
    ) -> String {
        let base = trait_id.name();
        if args.is_empty() {
            base.to_string()
        } else {
            let args = args
                .iter()
                .map(|arg| self.ty_name(*arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}[{args}]")
        }
    }

    pub(crate) fn trait_id_and_args(
        &self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if self.is_trait_def_id(*def_id) => {
                Some((TraitId::Source(*def_id), args.clone(), const_args.clone()))
            }
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) => Some((*trait_id, trait_args.clone(), trait_const_args.clone())),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
            }
            _ => None,
        }
    }

    pub(crate) fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.interner.primitive(primitive)
    }

    pub(crate) fn string_literal_type(&mut self, literal: &nia_ast::StringLiteral) -> InternedTyId {
        self.string_literal_array_type(literal)
    }

    pub(crate) fn string_literal_array_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = string_literal_char_len(literal).unwrap_or(0);
        let elem = self.primitive(PrimitiveTy::Char);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem,
        })
    }

    pub(crate) fn byte_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        self.byte_string_literal_array_type(literal)
    }

    pub(crate) fn byte_string_literal_array_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = byte_string_literal_len(literal).unwrap_or(0);
        let elem = self.primitive(PrimitiveTy::U8);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem,
        })
    }

    pub(crate) fn unit(&self) -> InternedTyId {
        self.interner.intern(TyKind::Tuple(Vec::new()))
    }

    pub(crate) fn never(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Never)
    }

    pub(crate) fn is_unit(&self, ty: InternedTyId) -> bool {
        self.interner
            .get(self.normalization.normalize(ty))
            .is_some_and(TyKind::is_unit)
    }

    pub(crate) fn is_never(&self, ty: InternedTyId) -> bool {
        self.normalization.normalize(ty) == self.never()
    }

    pub(crate) fn is_invalid_temporary_type(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Never))
        )
    }

    pub(crate) fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            )) | Some(TyKind::Vector {
                elem: PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool,
                ..
            })
        )
    }

    pub(crate) fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    pub(crate) fn is_u32(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        )
    }

    pub(crate) fn is_numeric(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.interner.get(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                    | Some(TyKind::Vector {
                        elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                        ..
                    })
            )
    }

    pub(crate) fn vector_bool_mask(&mut self, ty: InternedTyId) -> Option<InternedTyId> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Vector { lanes, .. }) = self.interner.get(ty).cloned() else {
            return None;
        };
        Some(self.interner.intern(TyKind::Vector {
            elem: PrimitiveTy::Bool,
            lanes,
        }))
    }

    pub(crate) fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. }
            )
        )
    }

    pub(crate) fn is_pointer_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Usize | PrimitiveTy::Isize))
        )
    }

    pub(crate) fn is_enum(&self, ty: InternedTyId) -> bool {
        self.enum_global_def_id(ty).is_some()
    }

    pub(crate) fn is_open_enum(&self, ty: InternedTyId) -> bool {
        let Some(enum_id) = self.enum_global_def_id(ty) else {
            return false;
        };
        if enum_id.module_id == self.defs.module_id {
            self.signatures
                .enums
                .get(&enum_id.def_id)
                .is_some_and(|signature| signature.is_open)
        } else {
            self.program_signature_scope
                .enum_(enum_id)
                .is_some_and(|program_enum| program_enum.signature.is_open)
        }
    }

    pub(crate) fn bool(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Bool)
    }

    pub(crate) fn i32(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::I32)
    }

    pub(crate) fn f64(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::F64)
    }

    pub(crate) fn error(&self) -> InternedTyId {
        self.interner.error()
    }
}

fn numeric_literal_suffix_for_expr(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Float(text) => numeric_literal_suffix(text),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => numeric_literal_suffix_for_expr(expr),
        _ => None,
    }
}
