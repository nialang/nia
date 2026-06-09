// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_sema_ir::{TraitObjectCoercion, TraitObjectUpcast};
use nia_span::Span;
use nia_trait_solve::{TraitGoal, TraitSolverContext};
use nia_ty::{AssociatedTypeBindingTy, TraitId, TyKind};

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
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::TraitObject {
            is_readonly: expected_const,
            trait_id: expected_trait,
            trait_args: expected_args,
            associated_type_bindings: expected_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::TraitObject {
            is_readonly: actual_const,
            trait_id: actual_trait,
            trait_args: actual_args,
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
                &expected_bindings,
                &actual_bindings,
            )
            || !self.trait_object_has_supertrait(
                actual_trait,
                &actual_args,
                expected_trait,
                &expected_args,
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
            associated_type_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::Pointer {
            is_readonly: actual_const,
            elem: self_ty,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if !expected_const && actual_const {
            return None;
        }
        if !self.is_object_safe_trait_object(
            expr.span,
            trait_id,
            &trait_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        if !self.trait_object_bindings_match_impl(
            self_ty,
            trait_id,
            &trait_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        self.record_trait_object_node_coercion(
            expr,
            TraitObjectCoercion {
                source_ty: actual,
                target_ty: expected,
            },
        );
        self.record_trait_object_vtable_instantiations(expr.span, self_ty, trait_id, &trait_args);
        Some(expected)
    }

    pub(crate) fn trait_object_coercion_self_ty(
        &mut self,
        source_ty: InternedTyId,
    ) -> InternedTyId {
        let source_ty = self.normalization.normalize(source_ty);
        match self.interner.get(source_ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) => elem,
            _ => source_ty,
        }
    }

    pub(crate) fn trait_object_bindings_match_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        associated_type_bindings: &[AssociatedTypeBindingTy],
    ) -> bool {
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
        let proven = {
            let mut solver = context.solver_with_associated_type_assumptions(
                &mut self.interner,
                &assumptions,
                &associated_type_assumptions,
            );
            solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.to_vec(),
            })
        };
        if !proven {
            return false;
        }
        associated_type_bindings.iter().all(|binding| {
            let binding_trait_id = binding.trait_id.unwrap_or(trait_id);
            let binding_trait_args = if binding.trait_id.is_some() {
                &binding.trait_args
            } else {
                trait_args
            };
            self.resolve_associated_type_projection(
                self_ty,
                binding_trait_id,
                binding_trait_args,
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
                let mut args = vec![self_ty];
                args.extend(trait_args.iter().copied());
                self.record_generic_instantiation(method_id, &args, span);
            }
        }
    }

    fn trait_object_impl_method_instance(
        &mut self,
        trait_id: GlobalDefId,
        method_name: &str,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let mut matches = Vec::new();
        for method in self.program_extension_methods.all_methods() {
            if method.name != method_name {
                continue;
            }
            if method.trait_id != Some(nia_ty::TraitId::Source(trait_id))
                || method.trait_args.len() != trait_args.len()
            {
                continue;
            }
            let Some(impl_signature) = self.program_trait_impls.iter().find(|impl_signature| {
                impl_signature.module_id == method.def_id.module_id
                    && impl_signature.local_index == method.impl_index
            }) else {
                continue;
            };
            let impl_interner = impl_signature.interner.clone();
            let target_ty =
                nia_ty::import_type_into(&mut self.interner, &impl_interner, method.target_ty);
            let mut substitutions = std::collections::HashMap::new();
            if !self.match_type_pattern(target_ty, self_ty, &mut substitutions) {
                continue;
            }
            let method_trait_args = method
                .trait_args
                .iter()
                .map(|arg| nia_ty::import_type_into(&mut self.interner, &impl_interner, *arg))
                .collect::<Vec<_>>();
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
                .impl_generics
                .iter()
                .filter_map(|generic| substitutions.get(generic).copied())
                .collect::<Vec<_>>();
            matches.push((method.def_id, args));
        }
        match matches.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(crate) fn is_object_safe_trait_object(
        &mut self,
        span: Span,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        associated_type_bindings: &[AssociatedTypeBindingTy],
    ) -> bool {
        let nia_ty::TraitId::Source(source_trait_id) = trait_id else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                "builtin trait objects are not supported yet",
            ));
            return false;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                "trait object refers to unknown trait",
            ));
            return false;
        };
        let mut ok = true;
        let self_ty = self
            .interner
            .intern(TyKind::GenericParam("Self".to_string()));
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
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.check_object_safe_type(span, elem);
            }
            Some(TyKind::Array { elem, .. }) => self.check_object_safe_type(span, elem),
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
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                self.is_object_safe_trait_object(
                    span,
                    trait_id,
                    &trait_args,
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
                | TyKind::ComptimeOnly
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_),
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
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has no receiver",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *check.ok = false;
            }
            if !method.signature.generics.is_empty() {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has method generics",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *check.ok = false;
            }
            if method
                .signature
                .params
                .first()
                .is_some_and(|param| param.receiver == Some(nia_ast::ReceiverKind::Value))
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` takes `self` by value",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *check.ok = false;
            }
            let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
            for param in method.signature.params.iter().skip(1) {
                let ty = self.substitute_generics(param.ty, &substitutions);
                let ty = self.object_safe_ty(check, ty);
                if self.type_mentions_self(ty, check.self_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at("E0301", 
                        check.span,
                        format!(
                            "trait `{}` is not object safe because method `{}` mentions `Self` outside the receiver",
                            self.nominal_ty_name(trait_id, trait_args),
                            method.name
                        ),
                    ));
                    *check.ok = false;
                }
            }
            let return_type =
                self.substitute_generics(method.signature.return_type, &substitutions);
            let return_type = self.object_safe_ty(check, return_type);
            if self.type_mentions_self(return_type, check.self_ty) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    check.span,
                    format!(
                        "trait `{}` is not object safe because method `{}` returns `Self`",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *check.ok = false;
            }
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics(*supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
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
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
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
                    .map(|arg| self.object_safe_ty(check, arg))
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
                        name: binding.name,
                        ty: self.object_safe_ty(check, binding.ty),
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
                    .map(|arg| self.object_safe_ty(check, arg))
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
                        name: binding.name,
                        ty: self.object_safe_ty(check, binding.ty),
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
                let self_ty = self.object_safe_ty(check, self_ty);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.object_safe_ty(check, arg))
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                })
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
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => self.type_mentions_self(elem, self_ty),
            Some(TyKind::Array { elem, .. }) => self.type_mentions_self(elem, self_ty),
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
            Some(
                TyKind::Error
                | TyKind::ComptimeOnly
                | TyKind::Primitive(_)
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
        target_trait: TraitId,
        target_args: &[InternedTyId],
    ) -> bool {
        self.trait_object_has_supertrait_inner(
            source_trait,
            source_args,
            target_trait,
            target_args,
            &mut Vec::new(),
        )
    }

    fn trait_object_has_supertrait_inner(
        &mut self,
        source_trait: TraitId,
        source_args: &[InternedTyId],
        target_trait: TraitId,
        target_args: &[InternedTyId],
        visited: &mut Vec<(TraitId, Vec<InternedTyId>)>,
    ) -> bool {
        if source_trait == target_trait
            && source_args.len() == target_args.len()
            && source_args
                .iter()
                .zip(target_args.iter())
                .all(|(source, target)| self.types_match(*source, *target))
        {
            return true;
        }
        if visited
            .iter()
            .any(|(trait_id, args)| *trait_id == source_trait && args == source_args)
        {
            return false;
        }
        visited.push((source_trait, source_args.to_vec()));
        match source_trait {
            TraitId::Builtin(trait_id) => trait_id.supertraits().iter().any(|supertrait| {
                let supertrait_args = if supertrait.preserves_trait_args {
                    source_args.to_vec()
                } else {
                    Vec::new()
                };
                self.trait_object_has_supertrait_inner(
                    TraitId::Builtin(supertrait.trait_id),
                    &supertrait_args,
                    target_trait,
                    target_args,
                    visited,
                )
            }),
            TraitId::Source(source_trait_id) => {
                let Some(signature) = self.resolved_trait_signature(source_trait_id) else {
                    return false;
                };
                let substitutions = self.generic_substitutions(&signature.generics, source_args);
                signature.supertraits.iter().any(|supertrait| {
                    let supertrait = self.substitute_generics(*supertrait, &substitutions);
                    let supertrait = self.normalization.normalize(supertrait);
                    match self.interner.get(supertrait).cloned() {
                        Some(TyKind::Nominal { def_id, args }) => self
                            .trait_object_has_supertrait_inner(
                                TraitId::Source(def_id),
                                &args,
                                target_trait,
                                target_args,
                                visited,
                            ),
                        Some(TyKind::BuiltinTrait { trait_id, args }) => self
                            .trait_object_has_supertrait_inner(
                                TraitId::Builtin(trait_id),
                                &args,
                                target_trait,
                                target_args,
                                visited,
                            ),
                        _ => false,
                    }
                })
            }
        }
    }
}
