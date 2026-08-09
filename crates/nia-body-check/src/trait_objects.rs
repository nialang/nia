// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_sema_ir::{PointerArrayToSliceCoercion, TraitObjectCoercion, TraitObjectUpcast};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_trait_solve::{TraitGoal, TraitSolverContext};
use nia_ty::{AssociatedTypeBindingTy, TraitId, TyKind};

#[derive(Clone)]
struct TraitObjectImplMethodMatch {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    target_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

struct TraitObjectTraitRef {
    trait_id: TraitId,
    args: Vec<InternedTyId>,
    const_args: Vec<nia_ty::ConstGenericArg>,
}

struct ObjectSafetyCheck<'a> {
    span: Span,
    // A synthetic `Self` marker lets object-safety checks detect uses of
    // receiver Self in parameter and return positions after generic
    // substitution, without coupling the check to source spelling.
    self_ty: InternedTyId,
    object_trait_id: nia_ty::TraitId,
    object_trait_args: Vec<InternedTyId>,
    associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    visiting: &'a mut Vec<nia_ids::GlobalDefId>,
    ok: &'a mut bool,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn coerce_trait_object_to_supertrait(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalize_aliases_in_type(actual);
        let Some(TyKind::TraitObject {
            is_readonly: expected_const,
            trait_id: expected_trait,
            trait_args: expected_args,
            trait_const_args: expected_const_args,
            associated_type_bindings: expected_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::TraitObject {
            is_readonly: actual_const,
            trait_id: actual_trait,
            trait_args: actual_args,
            trait_const_args: actual_const_args,
            associated_type_bindings: actual_bindings,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if self.types_match(expected, actual)
            || expected_const != actual_const
            || !self.trait_object_upcast_bindings_match(
                expected_trait,
                &expected_args,
                &expected_const_args,
                &expected_bindings,
                &actual_bindings,
            )
            || !self.trait_object_has_supertrait(
                actual_trait,
                &actual_args,
                &actual_const_args,
                expected_trait,
                &expected_args,
                &expected_const_args,
            )
        {
            return None;
        }
        self.record_trait_object_node_upcast(
            expr,
            TraitObjectUpcast {
                source_ty: actual,
                target_ty: expected,
            },
        );
        Some(expected)
    }

    fn trait_object_upcast_bindings_match(
        &mut self,
        target_trait: TraitId,
        target_args: &[InternedTyId],
        target_const_args: &[nia_ty::ConstGenericArg],
        target_bindings: &[AssociatedTypeBindingTy],
        source_bindings: &[AssociatedTypeBindingTy],
    ) -> bool {
        target_bindings.iter().all(|target_binding| {
            let effective_target_trait = target_binding.trait_id.unwrap_or(target_trait);
            let effective_target_args = if target_binding.trait_id.is_some() {
                &target_binding.trait_args
            } else {
                target_args
            };
            let effective_target_const_args = if target_binding.trait_id.is_some() {
                &target_binding.trait_const_args
            } else {
                target_const_args
            };
            source_bindings.iter().any(|source_binding| {
                source_binding.name == target_binding.name
                    && source_binding.trait_id.unwrap_or(target_trait) == effective_target_trait
                    && self.trait_args_match(
                        if source_binding.trait_id.is_some() {
                            &source_binding.trait_args
                        } else {
                            target_args
                        },
                        effective_target_args,
                    )
                    && self.const_generic_arg_slices_match(
                        if source_binding.trait_id.is_some() {
                            &source_binding.trait_const_args
                        } else {
                            target_const_args
                        },
                        effective_target_const_args,
                    )
                    && self.types_match(source_binding.ty, target_binding.ty)
            })
        })
    }

    pub(crate) fn coerce_pointer_to_trait_object(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::TraitObject {
            is_readonly: expected_const,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let (actual_const, self_ty) = match self.interner.get(actual).cloned() {
            Some(TyKind::Pointer { is_readonly, elem }) => (is_readonly, elem),
            Some(TyKind::Slice { is_readonly, elem }) => (
                is_readonly,
                self.interner.intern(TyKind::SlicePointee { elem }),
            ),
            _ => return None,
        };
        if !expected_const && actual_const {
            return None;
        }
        if !self.is_object_safe_trait_object(
            expr.span,
            trait_id,
            &trait_args,
            &trait_const_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        let self_ty = self.normalize_aliases_in_type(self_ty);
        if !self.trait_object_bindings_match_impl(
            self_ty,
            trait_id,
            &trait_args,
            &trait_const_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        self.record_trait_object_node_coercion(
            expr,
            TraitObjectCoercion {
                source_ty: actual,
                target_ty: expected,
                self_ty,
            },
        );
        self.record_trait_object_vtable_instantiations(expr.span, self_ty, trait_id, &trait_args);
        Some(expected)
    }

    pub(crate) fn coerce_method_receiver_to_trait_object(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual_receiver_ty: InternedTyId,
        receiver_kind: nia_ids::ReceiverKind,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::TraitObject {
            is_readonly: expected_const,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let source_ty = match receiver_kind {
            nia_ids::ReceiverKind::Value => return None,
            nia_ids::ReceiverKind::RefReadOnly => self.interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem: actual_receiver_ty,
            }),
            nia_ids::ReceiverKind::Ref => self.interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem: actual_receiver_ty,
            }),
        };
        let source_ty = self.normalization.normalize(source_ty);
        let (actual_const, self_ty) = match self.interner.get(source_ty).cloned() {
            Some(TyKind::Pointer { is_readonly, elem }) => (is_readonly, elem),
            _ => return None,
        };
        if !expected_const && actual_const {
            return None;
        }
        if !self.is_object_safe_trait_object(
            expr.span,
            trait_id,
            &trait_args,
            &trait_const_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        let self_ty = self.normalize_aliases_in_type(self_ty);
        if !self.trait_object_bindings_match_impl(
            self_ty,
            trait_id,
            &trait_args,
            &trait_const_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        self.record_trait_object_node_coercion(
            expr,
            TraitObjectCoercion {
                source_ty,
                target_ty: expected,
                self_ty,
            },
        );
        self.record_trait_object_vtable_instantiations(expr.span, self_ty, trait_id, &trait_args);
        Some(expected)
    }

    pub(crate) fn coerce_pointer_array_to_slice_trait_object(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let actual = self.normalization.normalize(actual);
        let (array_ty, slice_ty, slice_is_readonly) = self.pointer_array_slice_type(actual)?;
        self.coerce_pointer_to_trait_object(expr, expected, slice_ty)?;
        self.record_pointer_array_to_slice_node_coercion(
            expr,
            PointerArrayToSliceCoercion {
                pointer_ty: actual,
                array_ty,
                slice_ty,
                is_readonly: slice_is_readonly,
            },
        );
        Some(expected)
    }

    pub(crate) fn trait_object_bindings_match_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        associated_type_bindings: &[AssociatedTypeBindingTy],
    ) -> bool {
        let self_ty = self.normalize_aliases_in_type(self_ty);
        let assumptions = self.current_trait_goals();
        let associated_type_assumptions = self.current_associated_type_assumptions();
        let mut const_expr_values = self.const_expr_values_for_trait_solver(trait_const_args);
        for assumption in &assumptions {
            for arg in &assumption.trait_const_args {
                self.collect_const_expr_values_for_trait_solver(arg, &mut const_expr_values);
            }
        }
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
        let proven = {
            let mut solver = context.solver_with_associated_type_assumptions(
                &assumptions,
                &associated_type_assumptions,
            );
            solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.to_vec(),
                trait_const_args: trait_const_args.to_vec(),
            })
        };
        if !proven {
            self.record_trait_provider_demand(self_ty, trait_id);
            return false;
        }
        associated_type_bindings.iter().all(|binding| {
            let binding_trait_id = binding.trait_id.unwrap_or(trait_id);
            let binding_trait_args = if binding.trait_id.is_some() {
                &binding.trait_args
            } else {
                trait_args
            };
            let binding_trait_const_args = if binding.trait_id.is_some() {
                &binding.trait_const_args
            } else {
                trait_const_args
            };
            self.resolve_associated_type_projection(
                self_ty,
                binding_trait_id,
                binding_trait_args,
                binding_trait_const_args,
                &binding.name,
            )
            .is_some_and(|actual| self.types_match(actual, binding.ty))
        })
    }

    fn record_trait_object_vtable_instantiations(
        &mut self,
        span: Span,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
    ) {
        let nia_ty::TraitId::Source(source_trait_id) = trait_id else {
            return;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            return;
        };
        for method in &trait_signature.methods {
            let method_id = GlobalDefId {
                module_id: source_trait_id.module_id,
                def_id: method.def_id,
            };
            if let Some((def_id, args)) = self.trait_object_impl_method_instance(
                source_trait_id,
                &method.name,
                self_ty,
                trait_args,
            ) {
                self.record_generic_instantiation(def_id, &args, span);
            } else if method.has_default {
                let default_self_ty = self.trait_receiver_self_ty(self_ty).unwrap_or(self_ty);
                self.record_generic_instantiation_with_self_arg(
                    method_id,
                    Some(default_self_ty),
                    trait_args,
                    span,
                );
            }
        }
    }

    fn trait_object_impl_method_instance(
        &mut self,
        trait_id: GlobalDefId,
        method_name: &SymbolId,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let mut matches = Vec::new();
        let program_methods = self
            .program
            .extension_methods_named
            .map(|methods_named| methods_named(method_name))
            .unwrap_or_else(|| {
                self.program_extension_methods
                    .methods_named(method_name)
                    .cloned()
                    .collect()
            });
        for method in &program_methods {
            if &method.name != method_name {
                continue;
            }
            if method.trait_id != Some(nia_ty::TraitId::Source(trait_id))
                || method.trait_args.len() != trait_args.len()
            {
                continue;
            }
            if !self.program_trait_impls.iter().any(|impl_signature| {
                impl_signature.module_id == method.def_id.module_id
                    && impl_signature.impl_id == method.impl_id
            }) {
                continue;
            }
            let target_ty = method.target_ty;
            let mut substitutions = nia_symbol::SymbolMap::default();
            if !self.match_type_pattern(target_ty, self_ty, &mut substitutions) {
                continue;
            }
            let method_trait_args = method.trait_args.to_vec();
            if !method_trait_args
                .iter()
                .zip(trait_args)
                .all(|(pattern, actual)| {
                    self.match_type_pattern(*pattern, *actual, &mut substitutions)
                })
            {
                continue;
            }
            let args = method
                .effective_generics
                .iter()
                .filter_map(|generic| substitutions.get(generic).copied())
                .collect::<Vec<_>>();
            matches.push(TraitObjectImplMethodMatch {
                def_id: method.def_id,
                args,
                target_ty,
                trait_args: method_trait_args,
            });
        }
        let matches = self.filter_more_specific_trait_object_impl_methods(matches);
        match matches.as_slice() {
            [candidate] => Some((candidate.def_id, candidate.args.clone())),
            _ => None,
        }
    }

    fn filter_more_specific_trait_object_impl_methods(
        &mut self,
        matches: Vec<TraitObjectImplMethodMatch>,
    ) -> Vec<TraitObjectImplMethodMatch> {
        matches
            .iter()
            .filter(|candidate| {
                !matches.iter().any(|other| {
                    other.def_id != candidate.def_id
                        && self.trait_object_impl_method_more_specific(other, candidate)
                })
            })
            .cloned()
            .collect()
    }

    fn trait_object_impl_method_more_specific(
        &mut self,
        specific: &TraitObjectImplMethodMatch,
        general: &TraitObjectImplMethodMatch,
    ) -> bool {
        if specific.trait_args.len() != general.trait_args.len() {
            return false;
        }
        let target_subsumes = self.pattern_subsumes(general.target_ty, specific.target_ty);
        let mut any_strict = self.strictly_more_specific(specific.target_ty, general.target_ty);
        let args_subsume = specific.trait_args.iter().zip(&general.trait_args).all(
            |(specific_arg, general_arg)| {
                any_strict |= self.strictly_more_specific(*specific_arg, *general_arg);
                self.pattern_subsumes(*general_arg, *specific_arg)
            },
        );
        target_subsumes && args_subsume && any_strict
    }

    pub(crate) fn is_object_safe_trait_object(
        &mut self,
        span: Span,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        _trait_const_args: &[nia_ty::ConstGenericArg],
        associated_type_bindings: &[AssociatedTypeBindingTy],
    ) -> bool {
        let nia_ty::TraitId::Source(source_trait_id) = trait_id else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "builtin trait objects are not supported yet",
            ));
            return false;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "trait object refers to unknown trait",
            ));
            return false;
        };
        let mut ok = true;
        let self_ty = self.interner.intern(TyKind::SelfParam);
        let mut visiting = Vec::new();
        self.check_object_safe_trait_signature(
            &mut ObjectSafetyCheck {
                span,
                self_ty,
                object_trait_id: trait_id,
                object_trait_args: trait_args.to_vec(),
                associated_type_bindings: associated_type_bindings.to_vec(),
                visiting: &mut visiting,
                ok: &mut ok,
            },
            source_trait_id,
            &trait_signature,
            trait_args,
        );
        ok
    }

    pub(crate) fn check_object_safe_type(&mut self, span: Span, ty: InternedTyId) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.check_object_safe_type(span, elem);
            }
            Some(TyKind::Array { elem, .. }) => self.check_object_safe_type(span, elem),
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.check_object_safe_type(span, elem);
                }
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.check_object_safe_type(span, bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.check_object_safe_type(span, param);
                }
                self.check_object_safe_type(span, return_type);
            }
            Some(TyKind::Optional { elem }) => self.check_object_safe_type(span, elem),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.check_object_safe_type(span, error);
                self.check_object_safe_type(span, value);
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.check_object_safe_type(span, arg);
                }
            }
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                self.is_object_safe_trait_object(
                    span,
                    trait_id,
                    &trait_args,
                    &trait_const_args,
                    &associated_type_bindings,
                );
                for arg in trait_args {
                    self.check_object_safe_type(span, arg);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.check_object_safe_type(span, arg);
                    }
                    self.check_object_safe_type(span, binding.ty);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.check_object_safe_type(span, self_ty);
                for arg in trait_args {
                    self.check_object_safe_type(span, arg);
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_)
                | TyKind::SelfParam,
            )
            | None => {}
        }
    }

    fn check_object_safe_trait_signature(
        &mut self,
        check: &mut ObjectSafetyCheck<'_>,
        trait_id: nia_ids::GlobalDefId,
        trait_signature: &nia_item_signatures::TraitSignature,
        trait_args: &[InternedTyId],
    ) {
        if check.visiting.contains(&trait_id) {
            return;
        }
        check.visiting.push(trait_id);
        for method in &trait_signature.methods {
            if method
                .signature
                .params
                .first()
                .is_none_or(|param| param.receiver.is_none())
            {
                let method_name = self.symbol_name(method.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has no receiver",
                        self.nominal_ty_name(trait_id, trait_args),
                        method_name
                    ),
                ));
                *check.ok = false;
            }
            if !method.signature.generics.is_empty() {
                let method_name = self.symbol_name(method.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has method generics",
                        self.nominal_ty_name(trait_id, trait_args),
                        method_name
                    ),
                ));
                *check.ok = false;
            }
            if method
                .signature
                .params
                .first()
                .is_some_and(|param| param.receiver == Some(nia_ids::ReceiverKind::Value))
            {
                let method_name = self.symbol_name(method.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` takes `self` by value",
                        self.nominal_ty_name(trait_id, trait_args),
                        method_name
                    ),
                ));
                *check.ok = false;
            }
            let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
            for param in method.signature.params.iter().skip(1) {
                let ty = self.substitute_generics(param.ty, &substitutions);
                let ty = self.object_safe_ty(check, ty);
                if self.type_mentions_self(ty, check.self_ty) {
                    let method_name = self.symbol_name(method.name);
                    self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                        check.span,
                        format!(
                            "trait `{}` is not object safe because method `{}` mentions `Self` outside the receiver",
                            self.nominal_ty_name(trait_id, trait_args),
                            method_name
                        ),
                    ));
                    *check.ok = false;
                }
            }
            let return_type =
                self.substitute_generics(method.signature.return_type, &substitutions);
            let return_type = self.object_safe_ty(check, return_type);
            if self.type_mentions_self(return_type, check.self_ty) {
                let method_name = self.symbol_name(method.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` returns `Self`",
                        self.nominal_ty_name(trait_id, trait_args),
                        method_name
                    ),
                ));
                *check.ok = false;
            }
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics(supertrait.ty, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
                ..
            }) = self
                .interner
                .get(self.normalization.normalize(supertrait))
                .cloned()
            else {
                continue;
            };
            let Some(supertrait_signature) = self.resolved_trait_signature(supertrait_id) else {
                continue;
            };
            self.check_object_safe_trait_signature(
                check,
                supertrait_id,
                &supertrait_signature,
                &supertrait_args,
            );
        }
        check.visiting.pop();
    }

    fn object_safe_ty(&mut self, check: &ObjectSafetyCheck<'_>, ty: InternedTyId) -> InternedTyId {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
                ..
            }) if self_ty == check.self_ty => check
                .associated_type_bindings
                .iter()
                .find_map(|binding| {
                    let binding_trait_id = binding.trait_id.unwrap_or(check.object_trait_id);
                    let binding_trait_args = if binding.trait_id.is_some() {
                        &binding.trait_args
                    } else {
                        &check.object_trait_args
                    };
                    (binding.name == name
                        && binding_trait_id == trait_id
                        && self.trait_args_match(binding_trait_args, &trait_args))
                    .then_some(binding.ty)
                })
                .unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.object_safe_ty(check, elem))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.object_safe_ty(check, bound));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.object_safe_ty(check, param))
                    .collect();
                let return_type = self.object_safe_ty(check, return_type);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.object_safe_ty(check, elem);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.object_safe_ty(check, error);
                let value = self.object_safe_ty(check, value);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args, .. }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                self.interner.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args: Vec::new(),
                })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
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
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.object_safe_ty(check, arg.ty);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.object_safe_ty(check, arg))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.object_safe_ty(check, arg.ty);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.object_safe_ty(check, binding.ty),
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
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.object_safe_ty(check, arg.ty);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.object_safe_ty(check, arg))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.object_safe_ty(check, arg.ty);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.object_safe_ty(check, binding.ty),
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
                let self_ty = self.object_safe_ty(check, self_ty);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.object_safe_ty(check, arg.ty);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_)
                | TyKind::SelfParam,
            )
            | None => ty,
        }
    }

    fn trait_args_match(&mut self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.types_match(*left, *right))
    }

    fn type_mentions_self(&mut self, ty: InternedTyId, self_ty: InternedTyId) -> bool {
        let ty = self.normalization.normalize(ty);
        if ty == self_ty {
            return true;
        }
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => self.type_mentions_self(elem, self_ty),
            Some(TyKind::Array { elem, .. }) => self.type_mentions_self(elem, self_ty),
            Some(TyKind::Tuple(elems)) => elems
                .into_iter()
                .any(|elem| self.type_mentions_self(elem, self_ty)),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_mentions_self(bound, self_ty))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .into_iter()
                    .any(|param| self.type_mentions_self(param, self_ty))
                    || self.type_mentions_self(return_type, self_ty)
            }
            Some(TyKind::Optional { elem }) => self.type_mentions_self(elem, self_ty),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_mentions_self(error, self_ty) || self.type_mentions_self(value, self_ty)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => args
                .into_iter()
                .any(|arg| self.type_mentions_self(arg, self_ty)),
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
                    .into_iter()
                    .any(|arg| self.type_mentions_self(arg, self_ty))
                    || associated_type_bindings
                        .into_iter()
                        .any(|binding| self.type_mentions_self(binding.ty, self_ty))
            }
            Some(TyKind::Projection {
                self_ty: projection_self,
                trait_args,
                ..
            }) => {
                self.type_mentions_self(projection_self, self_ty)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_mentions_self(arg, self_ty))
            }
            Some(TyKind::SelfParam) => true,
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_),
            )
            | None => false,
        }
    }

    fn trait_object_has_supertrait(
        &mut self,
        source_trait: TraitId,
        source_args: &[InternedTyId],
        source_const_args: &[nia_ty::ConstGenericArg],
        target_trait: TraitId,
        target_args: &[InternedTyId],
        target_const_args: &[nia_ty::ConstGenericArg],
    ) -> bool {
        self.trait_object_has_supertrait_inner(
            TraitObjectTraitRef {
                trait_id: source_trait,
                args: source_args.to_vec(),
                const_args: source_const_args.to_vec(),
            },
            &TraitObjectTraitRef {
                trait_id: target_trait,
                args: target_args.to_vec(),
                const_args: target_const_args.to_vec(),
            },
            &mut Vec::new(),
        )
    }

    fn trait_object_has_supertrait_inner(
        &mut self,
        source: TraitObjectTraitRef,
        target: &TraitObjectTraitRef,
        visited: &mut Vec<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)>,
    ) -> bool {
        if source.trait_id == target.trait_id
            && source.args.len() == target.args.len()
            && self.const_generic_arg_slices_match(&source.const_args, &target.const_args)
            && source
                .args
                .iter()
                .zip(target.args.iter())
                .all(|(source, target)| self.types_match(*source, *target))
        {
            return true;
        }
        if visited.iter().any(|(trait_id, args, const_args)| {
            *trait_id == source.trait_id && args == &source.args && const_args == &source.const_args
        }) {
            return false;
        }
        visited.push((
            source.trait_id,
            source.args.clone(),
            source.const_args.clone(),
        ));
        match source.trait_id {
            TraitId::Builtin(trait_id) => trait_id.supertraits().iter().any(|supertrait| {
                let supertrait_args = if supertrait.preserves_trait_args {
                    source.args.clone()
                } else {
                    Vec::new()
                };
                self.trait_object_has_supertrait_inner(
                    TraitObjectTraitRef {
                        trait_id: TraitId::Builtin(supertrait.trait_id),
                        args: supertrait_args,
                        const_args: Vec::new(),
                    },
                    target,
                    visited,
                )
            }),
            TraitId::Source(source_trait_id) => {
                let Some(signature) = self.resolved_trait_signature(source_trait_id) else {
                    return false;
                };
                let (substitutions, const_substitutions) = self
                    .generic_substitutions_and_consts_for_def(
                        source_trait_id,
                        &source.args,
                        &source.const_args,
                    );
                signature.supertraits.iter().any(|supertrait| {
                    let supertrait = self.substitute_generics_and_consts(
                        supertrait.ty,
                        &substitutions,
                        &const_substitutions,
                    );
                    let supertrait = self.normalization.normalize(supertrait);
                    match self.interner.get(supertrait).cloned() {
                        Some(TyKind::Nominal {
                            def_id,
                            args,
                            const_args,
                        }) => self.trait_object_has_supertrait_inner(
                            TraitObjectTraitRef {
                                trait_id: TraitId::Source(def_id),
                                args,
                                const_args,
                            },
                            target,
                            visited,
                        ),
                        Some(TyKind::BuiltinTrait { trait_id, args }) => self
                            .trait_object_has_supertrait_inner(
                                TraitObjectTraitRef {
                                    trait_id: TraitId::Builtin(trait_id),
                                    args,
                                    const_args: Vec::new(),
                                },
                                target,
                                visited,
                            ),
                        _ => false,
                    }
                })
            }
        }
    }
}
