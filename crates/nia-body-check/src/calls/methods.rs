// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_defs::VisibleExtensionMethod;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, ReceiverKind, TraitId};
use nia_item_signatures::FunctionSignature;
use nia_sema_ir::{BracketSuffixResolution, BuiltinMethod, BuiltinOperatorOp, ResolvedCall};
use nia_span::Span;
use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, TyKind};

pub(super) struct MethodCall<'a> {
    pub(super) span: Span,
    pub(super) node_key: &'a nia_node_id::VersionedNodeKey,
    pub(super) receiver: &'a Expr,
    pub(super) receiver_ty: InternedTyId,
    pub(super) name: &'a str,
    pub(super) type_args: Option<&'a [BracketArg]>,
    pub(super) args: &'a [Expr],
    pub(super) expected: Option<InternedTyId>,
}

pub(super) struct MethodGenericContext<'a> {
    pub(super) span: Span,
    pub(super) target_substitutions: &'a HashMap<String, InternedTyId>,
    pub(super) method_args: Option<&'a [BracketArg]>,
    pub(super) lowered_method_args: &'a [InternedTyId],
    pub(super) expected: Option<InternedTyId>,
}

#[derive(Clone)]
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
    pub(super) target_substitutions: HashMap<String, InternedTyId>,
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
        let trait_candidates = if candidates.is_empty() {
            self.trait_method_candidates_for_receiver(receiver_ty, name)
        } else {
            Vec::new()
        };
        let dynamic_candidates = if candidates.is_empty() && trait_candidates.is_empty() {
            self.dynamic_trait_method_candidates_for_receiver(receiver_ty, name)
        } else {
            Vec::new()
        };
        let mut call_receiver_ty = receiver_ty;
        if candidates.is_empty() && trait_candidates.is_empty() && dynamic_candidates.is_empty() {
            BuiltinTraitMethod::from_name(name)?;
            call_receiver_ty = self
                .builtin_place_method_receiver_coercion(receiver, name, receiver_ty)
                .unwrap_or(receiver_ty);
        }
        self.check_method_call_with_receiver_ty(
            MethodCall {
                span,
                node_key: &expr.node_key,
                receiver,
                receiver_ty: call_receiver_ty,
                name,
                type_args: None,
                args,
                expected,
            },
            candidates,
            trait_candidates,
            dynamic_candidates,
        )
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
        let trait_candidates = if candidates.is_empty() {
            self.trait_method_candidates_for_receiver(receiver_ty, name)
        } else {
            Vec::new()
        };
        let dynamic_candidates = if candidates.is_empty() && trait_candidates.is_empty() {
            self.dynamic_trait_method_candidates_for_receiver(receiver_ty, name)
        } else {
            Vec::new()
        };
        let mut call_receiver_ty = receiver_ty;
        if candidates.is_empty() && trait_candidates.is_empty() && dynamic_candidates.is_empty() {
            BuiltinTraitMethod::from_name(name)?;
            call_receiver_ty = self
                .builtin_place_method_receiver_coercion(receiver, name, receiver_ty)
                .unwrap_or(receiver_ty);
        }
        self.check_method_call_with_receiver_ty(
            MethodCall {
                span,
                node_key: &expr.node_key,
                receiver,
                receiver_ty: call_receiver_ty,
                name,
                type_args: Some(type_args),
                args,
                expected,
            },
            candidates,
            trait_candidates,
            dynamic_candidates,
        )
    }

    fn check_method_call_with_receiver_ty(
        &mut self,
        call: MethodCall<'_>,
        candidates: Vec<MethodCandidate>,
        trait_candidates: Vec<TraitMethodCandidate>,
        dynamic_candidates: Vec<DynamicTraitMethodCandidate>,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.normalize_aliases_in_type(call.receiver_ty);
        if !dynamic_candidates.is_empty() {
            return self.check_dynamic_trait_method_call_with_receiver_ty(call, dynamic_candidates);
        }
        if candidates.is_empty() && !trait_candidates.is_empty() {
            return self.check_trait_method_call_with_receiver_ty(call, trait_candidates);
        }
        if candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_trait_method_call_with_receiver_ty(&call)
        {
            return Some(return_ty);
        }
        let viable_candidates = self.viable_method_candidates(&call, &candidates);
        let candidate = self.single_method_candidate(call.span, call.name, &viable_candidates)?;
        let method_id = candidate.method.def_id;
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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
                codes::TYPE_CHECK,
                call.span,
                "associated functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };

        let Some(receiver_kind) = receiver_param.receiver else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    codes::METHOD_RESOLUTION_INVARIANT,
                    "receiver method candidate has no receiver",
                )
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
                target_substitutions: &candidate.target_substitutions,
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
        let mut params: Vec<InternedTyId> = signature
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
            params = signature
                .params
                .iter()
                .skip(1)
                .map(|param| self.substitute_generics(param.ty, &substitutions))
                .collect();
        }
        self.check_where_predicates_hold(&signature.where_predicates, &substitutions, call.span);
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
    fn builtin_place_method_receiver_coercion(
        &mut self,
        receiver: &Expr,
        name: &str,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let method = BuiltinTraitMethod::from_name(name)?;
        if !method.is_place_method() {
            return None;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let Some(TyKind::Pointer {
            is_readonly,
            elem: array_ty,
        }) = self.interner.get(receiver_ty).cloned()
        else {
            return None;
        };
        let array_ty = self.normalization.normalize(array_ty);
        let Some(TyKind::Array { elem, .. }) = self.interner.get(array_ty).cloned() else {
            return None;
        };
        let slice_is_readonly = match method {
            BuiltinTraitMethod::Slice | BuiltinTraitMethod::GetPtr => {
                if is_readonly {
                    return None;
                }
                false
            }
            BuiltinTraitMethod::SliceRead | BuiltinTraitMethod::GetPtrRead => true,
            _ => return None,
        };
        let slice_ty = self.interner.intern(TyKind::Slice {
            is_readonly: slice_is_readonly,
            elem,
        });
        self.record_pointer_array_to_slice_node_coercion(
            receiver,
            nia_sema_ir::PointerArrayToSliceCoercion {
                pointer_ty: receiver_ty,
                array_ty,
                slice_ty,
                is_readonly: slice_is_readonly,
            },
        );
        Some(slice_ty)
    }
}
