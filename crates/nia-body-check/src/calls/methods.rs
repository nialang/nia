// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind, ReceiverKind};
use nia_defs::VisibleExtensionMethod;
use nia_diagnostic::Diagnostic;
use nia_ids::{BuiltinReceiverKind, BuiltinTraitMethod, GlobalDefId, InternedTyId, TraitId};
use nia_item_signatures::FunctionSignature;
use nia_sema_ir::{BracketSuffixResolution, BuiltinMethod, BuiltinOperatorOp, ResolvedCall};
use nia_span::Span;
use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, TyKind};

pub(super) struct MethodCall<'a> {
    pub(super) span: Span,
    pub(super) node_key: &'a nia_node_id::NodeKey,
    pub(super) receiver: &'a Expr,
    pub(super) receiver_ty: InternedTyId,
    pub(super) name: &'a str,
    pub(super) type_args: Option<&'a [BracketArg]>,
    pub(super) args: &'a [Expr],
    pub(super) expected: Option<InternedTyId>,
}

pub(super) struct MethodGenericContext<'a> {
    pub(super) span: Span,
    pub(super) receiver_ty: InternedTyId,
    pub(super) method_id: GlobalDefId,
    pub(super) method_args: Option<&'a [BracketArg]>,
    pub(super) lowered_method_args: &'a [InternedTyId],
    pub(super) expected: Option<InternedTyId>,
}

pub(super) struct TraitMethodCandidate {
    pub(super) trait_id: GlobalDefId,
    pub(super) method_id: GlobalDefId,
    pub(super) self_ty: InternedTyId,
    pub(super) trait_generics: Vec<String>,
    pub(super) trait_args: Vec<InternedTyId>,
    pub(super) signature: FunctionSignature,
    pub(super) has_default: bool,
}

pub(super) struct DynamicTraitMethodCandidate {
    pub(super) object_ty: InternedTyId,
    pub(super) trait_id: TraitId,
    pub(super) method_id: GlobalDefId,
    pub(super) trait_generics: Vec<String>,
    pub(super) trait_args: Vec<InternedTyId>,
    pub(super) associated_type_bindings: Vec<nia_ty::AssociatedTypeBindingTy>,
    pub(super) signature: FunctionSignature,
    pub(super) slot: usize,
}

#[derive(Clone)]
pub(super) struct MethodCandidate {
    pub(super) target_ty: InternedTyId,
    pub(super) method: VisibleExtensionMethod,
}

mod associated;
mod builtin_traits;
mod resolution;
mod trait_methods;

impl<'a> BodyChecker<'a> {
    pub(super) fn check_field_method_call(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let receiver_ty = self.check_expr(receiver);
        let candidates = self.method_candidates_for_receiver(receiver_ty, name);
        let trait_candidates = self.trait_method_candidates_for_receiver(receiver_ty, name);
        let dynamic_candidates =
            self.dynamic_trait_method_candidates_for_receiver(receiver_ty, name);
        if candidates.is_empty() && trait_candidates.is_empty() && dynamic_candidates.is_empty() {
            BuiltinTraitMethod::from_name(name)?;
        }
        if !candidates.is_empty() {
            self.single_method_candidate(span, name, candidates)?;
        }
        self.check_method_call_with_receiver_ty(MethodCall {
            span,
            node_key: &expr.node_key,
            receiver,
            receiver_ty,
            name,
            type_args: None,
            args,
            expected,
        })
    }

    pub(super) fn check_explicit_generic_field_method_call(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &str,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let receiver_ty = self.check_expr(receiver);
        let candidates = self.method_candidates_for_receiver(receiver_ty, name);
        if candidates.is_empty()
            && self
                .trait_method_candidates_for_receiver(receiver_ty, name)
                .is_empty()
            && self
                .dynamic_trait_method_candidates_for_receiver(receiver_ty, name)
                .is_empty()
            && BuiltinTraitMethod::from_name(name).is_none()
        {
            return None;
        }
        if !candidates.is_empty() {
            self.single_method_candidate(span, name, candidates)?;
        }
        self.check_method_call_with_receiver_ty(MethodCall {
            span,
            node_key: &expr.node_key,
            receiver,
            receiver_ty,
            name,
            type_args: Some(type_args),
            args,
            expected,
        })
    }

    fn check_method_call_with_receiver_ty(&mut self, call: MethodCall<'_>) -> Option<InternedTyId> {
        let receiver_ty = self.normalize_aliases_in_type(call.receiver_ty);
        let dynamic_candidates =
            self.dynamic_trait_method_candidates_for_receiver(receiver_ty, call.name);
        if !dynamic_candidates.is_empty() {
            return self.check_dynamic_trait_method_call_with_receiver_ty(call, dynamic_candidates);
        }
        let candidates = self.method_candidates_for_receiver(receiver_ty, call.name);
        let trait_candidates = if candidates.is_empty() {
            self.trait_method_candidates_for_receiver(receiver_ty, call.name)
        } else {
            Vec::new()
        };
        if candidates.is_empty() && !trait_candidates.is_empty() {
            return self.check_trait_method_call_with_receiver_ty(call, trait_candidates);
        }
        if candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_trait_method_call_with_receiver_ty(&call)
        {
            return Some(return_ty);
        }
        let method_id = self.single_method_candidate(call.span, call.name, candidates)?;
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                call.span,
                "method signature not found",
            ));
            return Some(self.error());
        };
        let Some(receiver_param) = signature
            .params
            .first()
            .filter(|param| param.receiver.is_some())
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                call.span,
                "associated functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };

        let Some(receiver_kind) = receiver_param.receiver else {
            self.diagnostics.push(
                Diagnostic::internal_error("I0106", "receiver method candidate has no receiver")
                    .primary(
                        call.span,
                        "method resolution selected a candidate without receiver metadata",
                    )
                    .debug("method_id", method_id)
                    .finish(),
            );
            return Some(self.error());
        };
        self.check_receiver_match(call.receiver, receiver_ty, receiver_kind);

        let Some(method_instantiation_args) = self.lowered_method_type_args(call.type_args) else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let Some(mut substitutions) = self.method_generic_substitutions(
            MethodGenericContext {
                span: call.span,
                receiver_ty,
                method_id,
                method_args: call.type_args,
                lowered_method_args: &method_instantiation_args,
                expected: call.expected,
            },
            &signature,
        ) else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let params: Vec<InternedTyId> = signature
            .params
            .iter()
            .skip(1)
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        if call.type_args.is_none() {
            self.infer_method_generics_from_args(call.args, &params, &mut substitutions);
            if !self.method_generics_are_complete(call.span, &signature, &substitutions) {
                self.check_call_arg_count(call.span, call.args.len(), params.len(), false);
                return Some(self.error());
            }
        }
        self.check_direct_call_args(call.span, call.args, &params, false);
        let target_args = self.extension_target_instance_args(method_id, &substitutions);
        if !target_args.is_empty() || !method_instantiation_args.is_empty() {
            let mut instance_args = target_args;
            instance_args.extend(method_instantiation_args);
            self.record_generic_instantiation(method_id, &instance_args, call.span);
            self.record_resolved_node_call(
                call.span,
                call.node_key,
                ResolvedCall::Method {
                    def_id: method_id,
                    args: instance_args,
                    receiver_kind,
                },
            );
        } else {
            self.record_resolved_node_call(
                call.span,
                call.node_key,
                ResolvedCall::Method {
                    def_id: method_id,
                    args: Vec::new(),
                    receiver_kind,
                },
            );
        }
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        let return_type = self.normalize_projection(return_type);
        Some(self.normalize_aliases_in_type(return_type))
    }
}
