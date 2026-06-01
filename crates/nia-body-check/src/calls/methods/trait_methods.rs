// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(in crate::calls::methods) fn check_trait_method_call_with_receiver_ty(
        &mut self,
        call: MethodCall<'_>,
        candidates: Vec<TraitMethodCandidate>,
    ) -> Option<InternedTyId> {
        let candidate = match candidates.as_slice() {
            [candidate] => candidate,
            [] => return None,
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    call.span,
                    format!("ambiguous trait method `{}`", call.name),
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
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "associated trait functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };
        self.check_receiver_match(call.receiver, call.receiver_ty, receiver_kind);
        let Some(method_instantiation_args) = self.lowered_method_type_args(call.type_args) else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        if call.type_args.is_some()
            && candidate.signature.generics.len() != method_instantiation_args.len()
        {
            self.diagnostics.push(Diagnostic::error(
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
        substitutions.insert("Self".to_string(), candidate.self_ty);
        if call.type_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(
                    &candidate.signature.generics,
                    &method_instantiation_args,
                ),
            );
        } else if let Some(expected) = call.expected {
            self.infer_generics_from_type(
                candidate.signature.return_type,
                expected,
                &mut substitutions,
                call.span,
            );
        }
        let params: Vec<InternedTyId> = candidate
            .signature
            .params
            .iter()
            .skip(1)
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        if call.type_args.is_none() {
            self.infer_method_generics_from_args(call.args, &params, &mut substitutions);
            if !self.method_generics_are_complete(call.span, &candidate.signature, &substitutions) {
                self.check_call_arg_count(call.span, call.args.len(), params.len(), false);
                return Some(self.error());
            }
        }
        self.check_direct_call_args(call.span, call.args, &params, false);
        if candidate.has_default {
            let mut instance_args = vec![candidate.self_ty];
            instance_args.extend(candidate.trait_args.iter().copied());
            instance_args.extend(method_instantiation_args.iter().copied());
            self.record_generic_instantiation(candidate.method_id, &instance_args, call.span);
        }
        self.record_resolved_call(
            call.span,
            ResolvedCall::TraitMethod {
                trait_id: candidate.trait_id,
                method_id: candidate.method_id,
                method_name: call.name.to_string(),
                self_ty: candidate.self_ty,
                trait_args: candidate.trait_args.clone(),
                args: method_instantiation_args,
            },
        );
        let return_type = self.substitute_generics(candidate.signature.return_type, &substitutions);
        Some(self.normalize_projection(return_type))
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
                self.diagnostics.push(Diagnostic::error(
                    call.span,
                    format!("ambiguous dynamic trait method `{}`", call.name),
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
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "associated trait functions are not supported by trait object method call syntax",
            ));
            return Some(self.error());
        };
        if receiver_kind == ReceiverKind::Value {
            self.diagnostics.push(Diagnostic::error(
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
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "trait object methods cannot take method generic arguments",
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let mut substitutions =
            self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
        substitutions.insert(
            "Self".to_string(),
            self.trait_object_self_ty(candidate.object_ty),
        );
        let params: Vec<InternedTyId> = candidate
            .signature
            .params
            .iter()
            .skip(1)
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        self.check_direct_call_args(call.span, call.args, &params, false);
        let return_type = self.substitute_generics(candidate.signature.return_type, &substitutions);
        self.record_resolved_call(
            call.span,
            ResolvedCall::DynamicTraitMethod {
                object_ty: candidate.object_ty,
                trait_id: candidate.trait_id,
                method_id: candidate.method_id,
                method_name: call.name.to_string(),
                trait_args: candidate.trait_args.clone(),
                slot: candidate.slot,
                params,
                return_type,
            },
        );
        Some(self.normalize_projection(return_type))
    }

    fn check_dynamic_trait_object_receiver_match(
        &mut self,
        call: &MethodCall<'_>,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind != ReceiverKind::Ref {
            return;
        }
        let Some(TyKind::TraitObject { is_const, .. }) = self.interner.get(call.receiver_ty) else {
            return;
        };
        if *is_const {
            self.diagnostics.push(Diagnostic::error(
                call.receiver.span,
                "receiver cannot be matched through `&const Trait`",
            ));
        }
    }
}
