// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{
    byte_string_literal_len, c_string_literal_len, float_literal_text, has_numeric_literal_suffix,
    integer_literal_value, integer_range, numeric_literal_suffix, parse_float_literal,
    string_literal_char_len,
};
use nia_ast::{Expr, ExprKind, TypeRef, UnaryOp};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, InternedTyId};
use nia_sema_ir::{ArrayToSliceCoercion, CStringPointerCoercion};
use nia_span::Span;
use nia_trait_solve::TraitSolverContext;
use nia_ty::{ArrayLenTy, AssociatedTypeBindingTy, PrimitiveTy, TraitId, TyKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectionNormalizationKey {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    name: String,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn optional_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::Optional { elem }) => Some(*elem),
            _ => None,
        }
    }

    pub(crate) fn error_union_parts(
        &self,
        ty: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId)> {
        match self.interner.get(self.normalization.normalize(ty)) {
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
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
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
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_projection_inner(elem, active_projections);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_projection_inner(error, active_projections);
                let value = self.normalize_projection_inner(value, active_projections);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
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
                        name: binding.name,
                        ty: self.normalize_projection_inner(binding.ty, active_projections),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
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
                        name: binding.name,
                        ty: self.normalize_projection_inner(binding.ty, active_projections),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.normalize_projection_inner(self_ty, active_projections);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active_projections))
                    .collect::<Vec<_>>();
                let key = ProjectionNormalizationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    name: name.clone(),
                };
                let projection = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    name: name.clone(),
                });
                if !active_projections.insert(key.clone()) {
                    return projection;
                }
                let normalized = self
                    .resolve_associated_type_projection(self_ty, trait_id, &trait_args, &name)
                    .map(|resolved| self.normalize_projection_inner(resolved, active_projections))
                    .unwrap_or(projection);
                active_projections.remove(&key);
                normalized
            }
            Some(
                TyKind::Error
                | TyKind::ComptimeOnly
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_),
            )
            | None => ty,
        }
    }

    pub(crate) fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let assumptions = self.current_trait_goals();
        let associated_type_assumptions = self.current_associated_type_assumptions();
        let context = TraitSolverContext {
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: &self.signatures.enums,
            program_enums: Some(self.program_enums),
        };
        let mut solver = context.solver_with_associated_type_assumptions(
            &mut self.interner,
            &assumptions,
            &associated_type_assumptions,
        );
        solver.resolve_associated_type(self_ty, trait_id, trait_args, name)
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
            "E0301",
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
        if let Some(coerced) = self.coerce_c_string_to_pointer(expr, expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_array_to_slice(expr, expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_mutable_pointer_to_readonly(expected, actual) {
            self.record_expr_node_type(expr, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_trait_object_to_supertrait(expr, expected, actual) {
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

    pub(crate) fn is_comptime_only_ty(&self, ty: InternedTyId) -> bool {
        matches!(self.interner.get(ty), Some(TyKind::ComptimeOnly))
    }

    pub(crate) fn array_expected_from_slice_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        match self.interner.get(expected) {
            Some(TyKind::Slice { elem, .. }) => Some(self.interner.intern(TyKind::Array {
                len: ArrayLenTy::Infer,
                elem: *elem,
            })),
            _ => None,
        }
    }

    pub(crate) fn coerce_array_to_slice(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Slice {
            is_readonly,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_readonly = *is_readonly;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        let actual_elem = *actual_elem;
        if !self.types_match(expected_elem, actual_elem) {
            return None;
        }
        self.check_reference_target_with_ty(
            expr,
            "array-to-slice source",
            is_readonly,
            Some(actual),
        );
        self.record_array_to_slice_node_coercion(
            expr,
            ArrayToSliceCoercion {
                array_ty: actual,
                slice_ty: expected,
                is_readonly,
            },
        );
        Some(expected)
    }

    pub(crate) fn coerce_c_string_to_pointer(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        if !matches!(expr.kind, ExprKind::CString(_)) {
            return None;
        }
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Pointer {
            is_readonly,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_readonly = *is_readonly;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        if !self.types_match(expected_elem, *actual_elem)
            || !self.types_match(expected_elem, self.primitive(PrimitiveTy::U8))
        {
            return None;
        }
        self.record_c_string_pointer_node_coercion(
            expr,
            CStringPointerCoercion {
                array_ty: actual,
                pointer_ty: expected,
                is_readonly,
            },
        );
        Some(expected)
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
                "E0301",
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
                "E0301",
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
                        "E0301",
                        expr.span,
                        format!("float literal `{text}` is out of range for F32 in {context}"),
                    ));
                }
                true
            }
            PrimitiveTy::F64 => {
                if !parse_float_literal::<f64>(text) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
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
        let suffix = numeric_literal_suffix_for_expr(expr).unwrap_or("<unknown>");
        self.diagnostics.push(Diagnostic::user_error_at(
            "E0301",
            expr.span,
            format!("invalid {kind} literal suffix `{suffix}`"),
        ));
    }

    pub(crate) fn expect_integer(&mut self, span: Span, actual: InternedTyId, context: &str) {
        if actual == self.error() || self.is_integer(actual) {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            "E0301",
            span,
            format!(
                "type mismatch in {context}: expected integer, got {}",
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn types_match(&self, expected: InternedTyId, actual: InternedTyId) -> bool {
        let mut checker = self.clone_for_type_compare();
        checker.types_match_normalized(expected, actual)
    }

    fn types_match_normalized(&mut self, expected: InternedTyId, actual: InternedTyId) -> bool {
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
                let Ok(expected_len) = self.array_len_value(Span::default(), &expected_len) else {
                    return false;
                };
                let Ok(actual_len) = self.array_len_value(Span::default(), &actual_len) else {
                    return false;
                };
                expected_len == actual_len
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
                    name: expected_name,
                }),
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    name: actual_name,
                }),
            ) => {
                expected_trait == actual_trait
                    && expected_name == actual_name
                    && expected_args.len() == actual_args.len()
                    && self.types_match_normalized(expected_self, actual_self)
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: expected_const,
                    trait_id: expected_trait,
                    trait_args: expected_args,
                    associated_type_bindings: expected_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: actual_const,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) => {
                expected_const == actual_const
                    && expected_trait == actual_trait
                    && expected_args.len() == actual_args.len()
                    && expected_bindings.len() == actual_bindings.len()
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
                    && expected_bindings.iter().all(|expected_binding| {
                        actual_bindings
                            .iter()
                            .find(|actual_binding| {
                                actual_binding.name == expected_binding.name
                                    && actual_binding.trait_id == expected_binding.trait_id
                                    && actual_binding.trait_args.len()
                                        == expected_binding.trait_args.len()
                                    && actual_binding
                                        .trait_args
                                        .iter()
                                        .zip(expected_binding.trait_args.iter())
                                        .all(|(actual, expected)| {
                                            self.types_match_normalized(*expected, *actual)
                                        })
                            })
                            .is_some_and(|actual_binding| {
                                self.types_match_normalized(expected_binding.ty, actual_binding.ty)
                            })
                    })
            }
            _ => false,
        }
    }

    fn clone_for_type_compare(&self) -> BodyChecker<'a> {
        BodyChecker {
            module: self.module,
            defs: self.defs,
            program: self.program,
            values: self.values,
            locals: self.locals,
            semantic_uses: self.semantic_uses,
            interner: self.interner.clone(),
            node_type_uses: self.node_type_uses,
            signatures: self.signatures,
            normalization: self.normalization,
            target: self.target,
            comptime: self.comptime,
            comptime_module: self.comptime_module,
            layouts: self.layouts,
            extensions: self.extensions,
            program_extension_methods: self.program_extension_methods,
            program_functions: self.program_functions,
            program_globals: self.program_globals,
            program_comptimes: self.program_comptimes,
            program_structs: self.program_structs,
            program_unions: self.program_unions,
            program_enums: self.program_enums,
            program_traits: self.program_traits,
            program_type_aliases: self.program_type_aliases,
            program_trait_impls: self.program_trait_impls,
            program_comptime: self.program_comptime,
            program_comptime_modules: self.program_comptime_modules,
            extension_methods_by_id: self.extension_methods_by_id.clone(),
            node_expr_types: HashMap::new(),
            node_bracket_suffix_resolutions: HashMap::new(),
            node_array_to_slice_coercions: HashMap::new(),
            node_c_string_pointer_coercions: HashMap::new(),
            node_trait_object_coercions: HashMap::new(),
            node_trait_object_upcasts: HashMap::new(),
            node_comptime_if_selections: HashMap::new(),
            node_builtin_values: HashMap::new(),
            node_array_repeat_counts: HashMap::new(),
            node_switch_pattern_values: HashMap::new(),
            node_resolved_calls: HashMap::new(),
            node_function_references: HashMap::new(),
            generic_instantiations: Vec::new(),
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
            local_types: HashMap::new(),
            global_types: HashMap::new(),
            comptime_types: HashMap::new(),
            method_receiver_kinds: HashMap::new(),
            traits_by_method_name: HashMap::new(),
            trait_impls_by_trait: HashMap::new(),
            diagnostics: Vec::new(),
            timing: self.timing,
            timing_module_id: self.timing_module_id,
            current_return: self.current_return,
            current_def_id: self.current_def_id,
            current_param_locals: self.current_param_locals.clone(),
            comptime_context_depth: self.comptime_context_depth,
            comptime_call_locals: Vec::new(),
        }
    }

    pub(crate) fn materialize_inferred_array_type(
        &self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        match (self.interner.get(expected), self.interner.get(actual)) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match(*expected_elem, *actual_elem) => Some(actual),
            _ => None,
        }
    }

    pub(crate) fn def_id_for_node(
        &mut self,
        node_key: &nia_node_id::NodeKey,
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

    pub(crate) fn ty_for_type(&self, ty: &TypeRef) -> InternedTyId {
        self.node_type_uses
            .get(&ty.node_key)
            .copied()
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
        let kind = self
            .interner
            .get(ty)
            .or_else(|| self.normalization.interner.get(ty))?;
        let TyKind::Nominal { def_id, args } = kind else {
            return None;
        };
        if def_id.module_id == self.defs.module_id {
            return self.layouts.nominal_type_layout(*def_id, args);
        }
        let layouts = self.program.layouts?.get(&def_id.module_id)?;
        layouts.nominal_type_layout(*def_id, args)
    }

    pub(crate) fn array_len_value(&self, span: Span, len: &ArrayLenTy) -> Result<u64, String> {
        match len {
            ArrayLenTy::ConstValue(value) => Ok(*value),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .ok_or_else(|| "array length was not evaluated by comptime".to_string()),
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
        }
    }

    pub(crate) fn ty_name(&self, ty: InternedTyId) -> String {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => primitive.name().to_string(),
            Some(TyKind::Vector { elem, lanes }) => format!("{}x{lanes}", elem.name()),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("&{mut_part}{}", self.ty_name(*elem))
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let mut_part = if *is_readonly { "" } else { "mut " };
                format!("&{mut_part}[{}]", self.ty_name(*elem))
            }
            Some(TyKind::SlicePointee { elem }) => format!("[{}]", self.ty_name(*elem)),
            Some(TyKind::Array { len, elem }) => {
                format!("[{}]{}", self.array_len_name(len), self.ty_name(*elem))
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
                let return_part = if self.is_void(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                format!("&fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::Nominal { def_id, args }) => self.nominal_ty_name(*def_id, args),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                self.builtin_trait_ty_name(*trait_id, args)
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
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
            }) => self.trait_pointee_ty_name(*trait_id, trait_args, associated_type_bindings),
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.ty_name(*self_ty);
                let trait_name = self.trait_ty_name(*trait_id, trait_args);
                format!("[{self_ty} as {trait_name}]::{name}")
            }
            Some(TyKind::GenericParam(name)) => name.clone(),
            Some(TyKind::ComptimeOnly) => "<comptime-only value>".to_string(),
            Some(TyKind::Error) => "<error type>".to_string(),
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
            ArrayLenTy::ConstValue(value) => value.to_string(),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<unevaluated comptime value>".to_string()),
            ArrayLenTy::Builtin { builtin, ty } => {
                format!("@{}[{}]()", builtin.name(), self.ty_name(*ty))
            }
        }
    }

    fn array_len_const_expr_value(&self, id: GlobalConstExprId) -> Option<u64> {
        if id.module_id == self.defs.module_id {
            return self.comptime.array_lengths.get(&id).copied();
        }
        self.program_comptime
            .get(&id.module_id)
            .and_then(|comptime| comptime.array_lengths.get(&id).copied())
    }

    pub(crate) fn nominal_ty_name(
        &self,
        def_id: nia_ids::GlobalDefId,
        args: &[InternedTyId],
    ) -> String {
        let base = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .map(|def| def.name.clone())
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
                .and_then(|defs| defs.defs.get(def_id.def_id))
                .map(|def| def.name.clone())
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
                    binding.name
                )
            } else {
                binding.name.clone()
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
                .and_then(|defs| defs.defs.get(def_id.def_id))
                .map(|def| def.name.clone())
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
                    binding.name
                )
            } else {
                binding.name.clone()
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
                .and_then(|defs| defs.defs.get(def_id.def_id))
                .map(|def| def.name.clone())
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
    ) -> Option<(TraitId, Vec<InternedTyId>)> {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::Nominal { def_id, args }) if self.is_trait_def_id(*def_id) => {
                Some((TraitId::Source(*def_id), args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.interner.primitive(primitive)
    }

    pub(crate) fn string_literal_type(&mut self, literal: &nia_ast::StringLiteral) -> InternedTyId {
        let len = string_literal_char_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::Char),
        })
    }

    pub(crate) fn byte_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = byte_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn c_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = c_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn void(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Void)
    }

    pub(crate) fn never(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Never)
    }

    pub(crate) fn is_void(&self, ty: InternedTyId) -> bool {
        ty == self.void()
    }

    pub(crate) fn is_never(&self, ty: InternedTyId) -> bool {
        self.normalization.normalize(ty) == self.never()
    }

    pub(crate) fn is_invalid_temporary_type(&self, ty: InternedTyId) -> bool {
        self.is_void(ty) || self.is_never(ty)
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
        let Some(TyKind::Vector { lanes, .. }) = self.interner.get(ty) else {
            return None;
        };
        Some(self.interner.intern(TyKind::Vector {
            elem: PrimitiveTy::Bool,
            lanes: *lanes,
        }))
    }

    pub(crate) fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
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
            self.program_enums
                .get(&enum_id)
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
