// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(in crate::calls::methods) fn check_trait_method_call_with_receiver_ty(
        &mut self,
        call: MethodCall<'_>,
        candidates: Vec<TraitMethodCandidate>,
    ) -> Option<InternedTyId> {
        let candidates = self.trait_method_candidates_matching_expected(&call, &candidates);
        let candidate = self.single_trait_method_candidate(call.span, call.name, &candidates)?;
        let Some(receiver_kind) = candidate
            .signature
            .params
            .first()
            .and_then(|param| param.receiver)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "associated trait functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };
        self.check_receiver_match(call.receiver, call.actual_receiver_ty, receiver_kind);
        let Some(method_instantiation_args) = self.lowered_method_type_args(call.type_args) else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        if call.type_args.is_some()
            && candidate.signature.generics.len() != method_instantiation_args.len()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                format!(
                    "generic argument count mismatch for trait method: expected {}, got {}",
                    candidate.signature.generics.len(),
                    method_instantiation_args.len()
                ),
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let mut substitutions =
            self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
        let mut const_substitutions = self.trait_const_substitutions_for_candidate(
            candidate.trait_id,
            &candidate.trait_args,
            &candidate.trait_const_args,
        );
        if call.type_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(
                    &candidate.signature.generics,
                    &method_instantiation_args,
                ),
            );
        } else if let Some(expected) = call.expected {
            let return_type = self.substitute_generics_and_consts_with_self(
                candidate.signature.return_type,
                &substitutions,
                &const_substitutions,
                candidate.self_ty,
            );
            let expected = self.normalize_projection(expected);
            self.infer_generics_from_type(return_type, expected, &mut substitutions, call.span);
        }
        let mut params: Vec<InternedTyId> = candidate
            .signature
            .params
            .iter()
            .skip(1)
            .map(|param| {
                self.substitute_generics_and_consts_with_self(
                    param.ty,
                    &substitutions,
                    &const_substitutions,
                    candidate.self_ty,
                )
            })
            .collect();
        if call.type_args.is_none() {
            self.infer_method_generics_from_args(call.args, &params, &mut substitutions);
            if !self.method_generics_are_complete(call.span, &candidate.signature, &substitutions) {
                self.check_call_arg_count(call.span, call.args.len(), params.len(), false);
                return Some(self.error());
            }
            params = candidate
                .signature
                .params
                .iter()
                .skip(1)
                .map(|param| {
                    self.substitute_generics_and_consts_with_self(
                        param.ty,
                        &substitutions,
                        &const_substitutions,
                        candidate.self_ty,
                    )
                })
                .collect();
        }
        self.check_direct_call_args(call.span, call.args, &params, false);
        let trait_args = candidate
            .trait_args
            .iter()
            .map(|arg| {
                self.substitute_generics_and_consts_with_self(
                    *arg,
                    &substitutions,
                    &const_substitutions,
                    candidate.self_ty,
                )
            })
            .collect::<Vec<_>>();
        if candidate.has_default {
            let default_self_ty = self
                .trait_receiver_self_ty(call.receiver_ty)
                .unwrap_or(candidate.self_ty);
            for (name, arg) in candidate
                .trait_generics
                .iter()
                .zip(candidate.trait_args.iter())
            {
                substitutions.insert(*name, *arg);
            }
            for (name, arg) in candidate
                .trait_generics
                .iter()
                .zip(candidate.trait_const_args.iter())
            {
                const_substitutions.insert(*name, arg.clone());
            }
            if let Some((instance_args, instance_const_args)) = self
                .complete_instance_args_and_const_args_for_def(
                    call.span,
                    candidate.method_id,
                    &substitutions,
                    &const_substitutions,
                )
            {
                self.record_generic_instantiation_with_self_and_const_args(
                    candidate.method_id,
                    Some(default_self_ty),
                    &instance_args,
                    &instance_const_args,
                    call.span,
                );
            }
        }
        self.record_resolved_node_call(
            call.span,
            call.node_key,
            ResolvedCall::TraitMethod {
                trait_id: candidate.trait_id,
                method_id: candidate.method_id,
                method_name: *call.name,
                self_ty: candidate.self_ty,
                trait_args,
                args: method_instantiation_args,
                receiver_kind,
            },
        );
        let return_type = self.substitute_generics_and_consts_with_self(
            candidate.signature.return_type,
            &substitutions,
            &const_substitutions,
            candidate.self_ty,
        );
        let return_type = self.normalize_projection(return_type);
        Some(self.normalize_aliases_in_type(return_type))
    }

    pub(in crate::calls::methods) fn check_dynamic_trait_method_call_with_receiver_ty(
        &mut self,
        call: MethodCall<'_>,
        candidates: Vec<DynamicTraitMethodCandidate>,
    ) -> Option<InternedTyId> {
        let candidate = match candidates.as_slice() {
            [candidate] => candidate,
            [] => return None,
            _ => {
                let name = self.symbol_name(*call.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    call.span,
                    format!("ambiguous dynamic trait method `{name}`"),
                ));
                return Some(self.error());
            }
        };
        let Some(receiver_kind) = candidate
            .signature
            .params
            .first()
            .and_then(|param| param.receiver)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "associated trait functions are not supported by trait object method call syntax",
            ));
            return Some(self.error());
        };
        if receiver_kind == ReceiverKind::Value {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "by-value trait object methods are not supported",
            ));
            return Some(self.error());
        }
        self.check_dynamic_trait_object_receiver_match(&call, receiver_kind);
        let Some(method_instantiation_args) = self.lowered_method_type_args(call.type_args) else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        if !method_instantiation_args.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "trait object methods cannot take method generic arguments",
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let substitutions =
            self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
        let const_substitutions = self.trait_const_substitutions_for_trait_id(
            candidate.trait_id,
            &candidate.trait_args,
            &candidate.trait_const_args,
        );
        let self_ty = self.trait_object_self_ty(candidate.object_ty);
        let params: Vec<InternedTyId> = candidate
            .signature
            .params
            .iter()
            .skip(1)
            .map(|param| {
                let ty = self.substitute_generics_and_consts_with_self(
                    param.ty,
                    &substitutions,
                    &const_substitutions,
                    self_ty,
                );
                self.normalize_dynamic_trait_object_projection(candidate, ty)
            })
            .collect();
        self.check_direct_call_args(call.span, call.args, &params, false);
        let return_type = self.substitute_generics_and_consts_with_self(
            candidate.signature.return_type,
            &substitutions,
            &const_substitutions,
            self_ty,
        );
        let return_type = self.normalize_dynamic_trait_object_projection(candidate, return_type);
        self.record_resolved_node_call(
            call.span,
            call.node_key,
            ResolvedCall::DynamicTraitMethod {
                object_ty: candidate.object_ty,
                trait_id: candidate.trait_id,
                method_id: candidate.method_id,
                method_name: *call.name,
                trait_args: candidate.trait_args.clone(),
                slot: candidate.slot,
                params,
                return_type,
                receiver_kind,
            },
        );
        let return_type = self.normalize_projection(return_type);
        Some(self.normalize_aliases_in_type(return_type))
    }

    fn normalize_dynamic_trait_object_projection(
        &mut self,
        candidate: &DynamicTraitMethodCandidate,
        ty: InternedTyId,
    ) -> InternedTyId {
        let ty = self.normalization.normalize(ty);
        let object_self_ty = self.trait_object_self_ty(candidate.object_ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
                ..
            }) if self.types_match(self_ty, object_self_ty)
                && trait_id == candidate.trait_id
                && self.trait_args_match_for_dynamic_object(&trait_args, &candidate.trait_args)
                && self.const_generic_arg_slices_match(
                    &trait_const_args,
                    &candidate.trait_const_args,
                ) =>
            {
                candidate
                    .associated_type_bindings
                    .iter()
                    .find_map(|binding| {
                        (binding.name == name
                            && binding
                                .trait_id
                                .is_none_or(|trait_id| trait_id == candidate.trait_id)
                            && (binding.trait_id.is_none()
                                || self.trait_args_match_for_dynamic_object(
                                    &binding.trait_args,
                                    &candidate.trait_args,
                                ))
                            && (binding.trait_id.is_none()
                                || self.const_generic_arg_slices_match(
                                    &binding.trait_const_args,
                                    &candidate.trait_const_args,
                                )))
                        .then_some(binding.ty)
                    })
                    .unwrap_or(ty)
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.normalize_dynamic_trait_object_projection(candidate, elem))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound
                    .map(|bound| self.normalize_dynamic_trait_object_projection(candidate, bound));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_dynamic_trait_object_projection(candidate, param))
                    .collect();
                let return_type =
                    self.normalize_dynamic_trait_object_projection(candidate, return_type);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_dynamic_trait_object_projection(candidate, elem);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_dynamic_trait_object_projection(candidate, error);
                let value = self.normalize_dynamic_trait_object_projection(candidate, value);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_dynamic_trait_object_projection(candidate, arg))
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
                    .map(|arg| self.normalize_dynamic_trait_object_projection(candidate, arg))
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
                    .map(|arg| self.normalize_dynamic_trait_object_projection(candidate, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_dynamic_trait_object_projection(candidate, arg.ty);
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
                            .map(|arg| {
                                self.normalize_dynamic_trait_object_projection(candidate, arg)
                            })
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self
                                    .normalize_dynamic_trait_object_projection(candidate, arg.ty);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_dynamic_trait_object_projection(candidate, binding.ty),
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
                    .map(|arg| self.normalize_dynamic_trait_object_projection(candidate, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_dynamic_trait_object_projection(candidate, arg.ty);
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
                            .map(|arg| {
                                self.normalize_dynamic_trait_object_projection(candidate, arg)
                            })
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self
                                    .normalize_dynamic_trait_object_projection(candidate, arg.ty);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_dynamic_trait_object_projection(candidate, binding.ty),
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
                let self_ty = self.normalize_dynamic_trait_object_projection(candidate, self_ty);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_dynamic_trait_object_projection(candidate, arg))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_dynamic_trait_object_projection(candidate, arg.ty);
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
                | TyKind::ClosureState { .. }
                | TyKind::SelfParam,
            )
            | None => ty,
        }
    }

    fn trait_const_substitutions_for_candidate(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> SymbolMap<ConstGenericArg> {
        self.generic_substitutions_and_consts_for_def(trait_id, trait_args, trait_const_args)
            .1
    }

    fn trait_const_substitutions_for_trait_id(
        &mut self,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> SymbolMap<ConstGenericArg> {
        let TraitId::Source(trait_id) = trait_id else {
            return SymbolMap::default();
        };
        self.generic_substitutions_and_consts_for_def(trait_id, trait_args, trait_const_args)
            .1
    }

    fn trait_args_match_for_dynamic_object(
        &mut self,
        left: &[InternedTyId],
        right: &[InternedTyId],
    ) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.types_match(*left, *right))
    }

    fn check_dynamic_trait_object_receiver_match(
        &mut self,
        call: &MethodCall<'_>,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind != ReceiverKind::Ref {
            return;
        }
        let Some(TyKind::TraitObject { is_readonly, .. }) = self.interner.get(call.receiver_ty)
        else {
            return;
        };
        if *is_readonly {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.receiver.span,
                "receiver cannot be matched through read-only `&Trait`",
            ));
        }
    }

    fn trait_method_candidates_matching_expected(
        &mut self,
        call: &MethodCall<'_>,
        candidates: &[TraitMethodCandidate],
    ) -> Vec<TraitMethodCandidate> {
        let Some(expected) = call.expected else {
            return candidates.to_vec();
        };
        let expected = self.normalize_projection(expected);
        let filtered = candidates
            .iter()
            .filter(|candidate| {
                let substitutions =
                    self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
                let return_type = self.substitute_generics_with_self(
                    candidate.signature.return_type,
                    &substitutions,
                    candidate.self_ty,
                );
                let return_type = self.normalize_projection(return_type);
                self.types_match(expected, return_type)
            })
            .cloned()
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            candidates.to_vec()
        } else {
            filtered
        }
    }
}

impl<'a> BodyChecker<'a> {
    fn single_trait_method_candidate(
        &mut self,
        span: Span,
        name: &SymbolId,
        candidates: &[TraitMethodCandidate],
    ) -> Option<TraitMethodCandidate> {
        let mut selected = None;
        let mut count = 0;
        for (index, candidate) in candidates.iter().enumerate() {
            if candidates.iter().enumerate().any(|(other_index, other)| {
                other_index != index && self.trait_method_candidate_more_specific(other, candidate)
            }) {
                continue;
            }
            selected = Some(candidate.clone());
            count += 1;
            if count <= 1 {
                continue;
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("ambiguous trait method `{}`", self.symbol_name(*name)),
            ));
            return None;
        }
        selected
    }

    fn trait_method_candidate_more_specific(
        &mut self,
        specific: &TraitMethodCandidate,
        general: &TraitMethodCandidate,
    ) -> bool {
        if specific.trait_id != general.trait_id
            || specific.trait_method_id != general.trait_method_id
            || specific.trait_args.len() != general.trait_args.len()
        {
            return false;
        }
        let mut specific_is_stricter =
            self.strictly_more_specific(specific.self_ty, general.self_ty);
        if !self.pattern_subsumes(general.self_ty, specific.self_ty) {
            return false;
        }
        for (specific_arg, general_arg) in specific.trait_args.iter().zip(&general.trait_args) {
            if !self.pattern_subsumes(*general_arg, *specific_arg) {
                return false;
            }
            if self.strictly_more_specific(*specific_arg, *general_arg) {
                specific_is_stricter = true;
            }
        }
        specific_is_stricter
    }
}
