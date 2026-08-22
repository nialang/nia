// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_defs::VisibleExtensionMethod;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, ReceiverKind, TraitId};
use nia_item_signatures::FunctionSignature;
use nia_sema_ir::{BracketSuffixResolution, BuiltinMethod, BuiltinOperatorOp, ResolvedCall};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, known};
use nia_ty::{ArrayLenTy, BuiltinTrait, ConstGenericArg, PrimitiveTy, TyKind};

pub(super) struct MethodCall<'a> {
    pub(super) span: Span,
    pub(super) node_key: &'a nia_node_id::VersionedNodeKey,
    pub(super) receiver: &'a Expr,
    pub(super) receiver_ty: InternedTyId,
    pub(super) actual_receiver_ty: InternedTyId,
    pub(super) name: &'a SymbolId,
    pub(super) type_args: Option<&'a [BracketArg]>,
    pub(super) args: &'a [Expr],
    pub(super) expected: Option<InternedTyId>,
}

pub(super) struct MethodGenericContext<'a> {
    pub(super) span: Span,
    pub(super) self_ty: InternedTyId,
    pub(super) target_substitutions: &'a SymbolMap<InternedTyId>,
    pub(super) target_const_substitutions: &'a SymbolMap<ConstGenericArg>,
    pub(super) method_args: Option<&'a [BracketArg]>,
    pub(super) lowered_method_args: &'a [InternedTyId],
    pub(super) lowered_method_const_args: &'a [ConstGenericArg],
    pub(super) expected: Option<InternedTyId>,
}

struct MethodReceiverResolution {
    receiver_ty: InternedTyId,
    candidates: Vec<MethodCandidate>,
    trait_candidates: Vec<TraitMethodCandidate>,
    dynamic_candidates: Vec<DynamicTraitMethodCandidate>,
    trait_candidates_searched: bool,
}

#[derive(Clone)]
pub(super) struct TraitMethodCandidate {
    pub(super) trait_id: GlobalDefId,
    pub(super) trait_method_id: GlobalDefId,
    pub(super) method_id: GlobalDefId,
    pub(super) self_ty: InternedTyId,
    pub(super) trait_generics: Vec<SymbolId>,
    pub(super) trait_args: Vec<InternedTyId>,
    pub(super) trait_const_args: Vec<ConstGenericArg>,
    pub(super) signature: FunctionSignature,
    pub(super) has_default: bool,
    pub(super) is_assumed: bool,
}

pub(super) struct DynamicTraitMethodCandidate {
    pub(super) object_ty: InternedTyId,
    pub(super) trait_id: TraitId,
    pub(super) method_id: GlobalDefId,
    pub(super) trait_generics: Vec<SymbolId>,
    pub(super) trait_args: Vec<InternedTyId>,
    pub(super) trait_const_args: Vec<ConstGenericArg>,
    pub(super) associated_type_bindings: Vec<nia_ty::AssociatedTypeBindingTy>,
    pub(super) signature: FunctionSignature,
    pub(super) slot: usize,
}

#[derive(Clone)]
pub(super) struct MethodCandidate {
    pub(super) target_ty: InternedTyId,
    pub(super) self_ty: InternedTyId,
    pub(super) method: VisibleExtensionMethod,
    pub(super) target_substitutions: SymbolMap<InternedTyId>,
    pub(super) target_const_substitutions: SymbolMap<ConstGenericArg>,
}

mod associated;
mod builtin_traits;
mod pattern_matching;
mod resolution;
mod trait_methods;
mod type_patterns;

impl<'a> BodyChecker<'a> {
    pub(super) fn check_field_method_call(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &SymbolId,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let receiver_ty = self.profile_stage("body_check.profile.method.receiver_expr", |this| {
            this.check_expr(receiver)
        });
        let resolution = self.method_receiver_resolution(receiver, receiver_ty, name);
        self.check_method_call_with_receiver_ty(
            MethodCall {
                span,
                node_key: &expr.node_key,
                receiver,
                receiver_ty: resolution.receiver_ty,
                actual_receiver_ty: receiver_ty,
                name,
                type_args: None,
                args,
                expected,
            },
            resolution.candidates,
            resolution.trait_candidates,
            resolution.dynamic_candidates,
            resolution.trait_candidates_searched,
        )
    }

    pub(super) fn check_explicit_generic_field_method_call(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &SymbolId,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let receiver_ty = self.profile_stage("body_check.profile.method.receiver_expr", |this| {
            this.check_expr(receiver)
        });
        let resolution = self.method_receiver_resolution(receiver, receiver_ty, name);
        self.check_method_call_with_receiver_ty(
            MethodCall {
                span,
                node_key: &expr.node_key,
                receiver,
                receiver_ty: resolution.receiver_ty,
                actual_receiver_ty: receiver_ty,
                name,
                type_args: Some(type_args),
                args,
                expected,
            },
            resolution.candidates,
            resolution.trait_candidates,
            resolution.dynamic_candidates,
            resolution.trait_candidates_searched,
        )
    }

    fn method_receiver_resolution(
        &mut self,
        receiver: &Expr,
        actual_receiver_ty: InternedTyId,
        name: &SymbolId,
    ) -> MethodReceiverResolution {
        let mut receiver_ty = actual_receiver_ty;
        let (mut candidates, mut trait_candidates, mut trait_candidates_searched) =
            self.method_candidates_and_traits(receiver_ty, name);
        let mut dynamic_receiver_ty = self.dynamic_trait_object_receiver_ty(receiver_ty);
        let mut dynamic_candidates = self.dynamic_method_candidates(dynamic_receiver_ty, name);

        if candidates.is_empty()
            && trait_candidates.is_empty()
            && dynamic_candidates.is_empty()
            && let Some(slice_ty) = self.pointer_array_method_receiver_slice_ty(receiver_ty)
        {
            let (slice_candidates, slice_trait_candidates, slice_traits_searched) =
                self.method_candidates_and_traits(slice_ty, name);
            let slice_dynamic_receiver_ty = self.dynamic_trait_object_receiver_ty(slice_ty);
            let slice_dynamic_candidates =
                self.dynamic_method_candidates(slice_dynamic_receiver_ty, name);
            if !slice_candidates.is_empty()
                || !slice_trait_candidates.is_empty()
                || !slice_dynamic_candidates.is_empty()
            {
                receiver_ty = slice_ty;
                candidates = slice_candidates;
                trait_candidates = slice_trait_candidates;
                trait_candidates_searched = slice_traits_searched;
                dynamic_receiver_ty = slice_dynamic_receiver_ty;
                dynamic_candidates = slice_dynamic_candidates;
            }
        }

        let mut call_receiver_ty = dynamic_receiver_ty.unwrap_or(receiver_ty);
        if candidates.is_empty()
            && trait_candidates.is_empty()
            && dynamic_candidates.is_empty()
            && (crate::symbols::builtin_trait_method_symbol(*name).is_some()
                || matches!(*name, known::PTR | known::PTR_MUT))
        {
            call_receiver_ty = self
                .builtin_method_receiver_coercion(receiver, name, actual_receiver_ty)
                .unwrap_or(receiver_ty);
        }
        MethodReceiverResolution {
            receiver_ty: call_receiver_ty,
            candidates,
            trait_candidates,
            dynamic_candidates,
            trait_candidates_searched,
        }
    }

    fn method_candidates_and_traits(
        &mut self,
        receiver_ty: InternedTyId,
        name: &SymbolId,
    ) -> (Vec<MethodCandidate>, Vec<TraitMethodCandidate>, bool) {
        let candidates = self.profile_stage("body_check.profile.method.candidates", |this| {
            this.method_candidates_for_receiver(receiver_ty, name)
        });
        if candidates.is_empty() {
            let trait_candidates = self
                .profile_stage("body_check.profile.method.trait_candidates", |this| {
                    this.trait_method_candidates_for_receiver(receiver_ty, name)
                });
            (candidates, trait_candidates, true)
        } else {
            let trait_candidates = self.profile_stage(
                "body_check.profile.method.assumed_trait_candidates",
                |this| this.assumed_trait_method_candidates_for_receiver(receiver_ty, name),
            );
            (candidates, trait_candidates, false)
        }
    }

    fn dynamic_method_candidates(
        &mut self,
        receiver_ty: Option<InternedTyId>,
        name: &SymbolId,
    ) -> Vec<DynamicTraitMethodCandidate> {
        receiver_ty
            .map(|object_ty| {
                self.profile_stage("body_check.profile.method.dynamic_candidates", |this| {
                    this.dynamic_trait_method_candidates_for_receiver(object_ty, name)
                })
            })
            .unwrap_or_default()
    }

    fn pointer_array_method_receiver_slice_ty(
        &mut self,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        self.pointer_array_slice_type(receiver_ty)
            .map(|(_, slice_ty, _)| slice_ty)
    }

    fn check_method_call_with_receiver_ty(
        &mut self,
        call: MethodCall<'_>,
        candidates: Vec<MethodCandidate>,
        trait_candidates: Vec<TraitMethodCandidate>,
        dynamic_candidates: Vec<DynamicTraitMethodCandidate>,
        trait_candidates_searched: bool,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.normalize_aliases_in_type(call.receiver_ty);
        let viable_candidates = self.profile_stage("body_check.profile.method.viable", |this| {
            this.viable_method_candidates(&call, &candidates)
        });
        let trait_candidates = if !trait_candidates_searched
            && trait_candidates.is_empty()
            && viable_candidates.is_empty()
        {
            self.profile_stage("body_check.profile.method.trait_candidates", |this| {
                this.trait_method_candidates_for_receiver(receiver_ty, call.name)
            })
        } else {
            trait_candidates
        };
        let (assumed_trait_candidates, visible_trait_candidates): (Vec<_>, Vec<_>) =
            trait_candidates
                .into_iter()
                .partition(|candidate| candidate.is_assumed);
        if !dynamic_candidates.is_empty() {
            return self.profile_stage("body_check.profile.method.dynamic_call", |this| {
                this.check_dynamic_trait_method_call_with_receiver_ty(call, dynamic_candidates)
            });
        }
        if !assumed_trait_candidates.is_empty() {
            return self.profile_stage("body_check.profile.method.trait_call", |this| {
                this.check_trait_method_call_with_receiver_ty(call, assumed_trait_candidates)
            });
        }
        if viable_candidates.is_empty() && !visible_trait_candidates.is_empty() {
            return self.profile_stage("body_check.profile.method.trait_call", |this| {
                this.check_trait_method_call_with_receiver_ty(call, visible_trait_candidates)
            });
        }
        if viable_candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_range_method(&call, receiver_ty)
        {
            return Some(return_ty);
        }
        if viable_candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_slice_pointer_method(&call, receiver_ty)
        {
            return Some(return_ty);
        }
        if viable_candidates.is_empty()
            && let Some(return_ty) = self
                .profile_stage("body_check.profile.method.builtin_trait_call", |this| {
                    this.check_builtin_trait_method_call_with_receiver_ty(&call)
                })
        {
            return Some(return_ty);
        }
        if viable_candidates.is_empty() && candidates.len() > 1 {
            let name = self.symbol_name(*call.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                format!("no matching method overload `{name}`"),
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if viable_candidates.is_empty() {
            self.record_method_provider_demand(receiver_ty, *call.name);
        }
        let candidate = self
            .profile_stage("body_check.profile.method.single_candidate", |this| {
                this.single_method_candidate(call.span, call.name, &viable_candidates)
            })?;
        let method_id = candidate.method.def_id;
        self.record_semantic_provider_module(method_id.module_id);
        let Some(signature) = self.profile_stage("body_check.profile.method.signature", |this| {
            this.resolved_function_signature(method_id)
                .map(|resolved| resolved.signature)
        }) else {
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
        let receiver_expected_ty = self.receiver_ty_for_target(candidate.target_ty, receiver_kind);
        let receiver_expected_ty = self.substitute_generics_and_consts_with_self(
            receiver_expected_ty,
            &candidate.target_substitutions,
            &candidate.target_const_substitutions,
            candidate.self_ty,
        );
        let receiver_expr_expected_ty = self.method_receiver_expr_expected_ty(
            receiver_expected_ty,
            call.actual_receiver_ty,
            receiver_kind,
        );
        if self
            .coerce_method_receiver_to_trait_object(
                call.receiver,
                receiver_expected_ty,
                call.actual_receiver_ty,
                receiver_kind,
            )
            .is_none()
        {
            self.expect_expr_type(
                call.receiver,
                receiver_expr_expected_ty,
                call.actual_receiver_ty,
                "receiver argument",
            );
        }
        self.check_receiver_match(call.receiver, call.actual_receiver_ty, receiver_kind);

        let Some((method_instantiation_args, method_const_args)) = self
            .profile_stage("body_check.profile.method.lower_type_args", |this| {
                this.lowered_method_type_args(call.type_args, &signature.generic_params)
            })
        else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let Some((mut substitutions, const_substitutions)) =
            self.profile_stage("body_check.profile.method.generic_substitutions", |this| {
                this.method_generic_substitutions(
                    MethodGenericContext {
                        span: call.span,
                        self_ty: candidate.self_ty,
                        target_substitutions: &candidate.target_substitutions,
                        target_const_substitutions: &candidate.target_const_substitutions,
                        method_args: call.type_args,
                        lowered_method_args: &method_instantiation_args,
                        lowered_method_const_args: &method_const_args,
                        expected: call.expected,
                    },
                    &signature,
                )
            })
        else {
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let mut params: Vec<InternedTyId> = signature
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
            self.profile_stage("body_check.profile.method.infer_args", |this| {
                this.infer_method_generics_from_args(call.args, &params, &mut substitutions);
            });
            if !self.method_generics_are_complete(call.span, &signature, &substitutions) {
                self.check_call_arg_count(call.span, call.args.len(), params.len(), false);
                return Some(self.error());
            }
            params = signature
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
        self.profile_stage("body_check.profile.method.infer_where", |this| {
            this.infer_method_generics_from_where_predicates(
                &signature,
                &candidate.method.where_predicates,
                &mut substitutions,
            );
        });
        self.profile_stage("body_check.profile.method.check_where", |this| {
            this.check_where_predicates_hold(
                &signature.where_predicates,
                &substitutions,
                &const_substitutions,
                call.span,
            );
            this.check_where_predicates_hold(
                &candidate.method.where_predicates,
                &substitutions,
                &const_substitutions,
                call.span,
            );
        });
        self.profile_stage("body_check.profile.method.check_args", |this| {
            this.check_direct_call_args(call.span, call.args, &params, false);
        });
        let Some((instance_args, const_instance_args)) =
            self.profile_stage("body_check.profile.method.instance_args", |this| {
                this.complete_instance_args_and_const_args_for_def(
                    call.span,
                    method_id,
                    &substitutions,
                    &const_substitutions,
                )
            })
        else {
            return Some(self.error());
        };
        if !instance_args.is_empty() || !const_instance_args.is_empty() {
            self.record_generic_instantiation_with_const_args(
                method_id,
                &instance_args,
                &const_instance_args,
                call.span,
            );
            self.record_resolved_node_call(
                call.span,
                call.node_key,
                ResolvedCall::Method {
                    def_id: method_id,
                    args: instance_args,
                    const_args: const_instance_args,
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
                    const_args: Vec::new(),
                    receiver_kind,
                },
            );
        }
        self.profile_stage("body_check.profile.method.return_type", |this| {
            let return_type = this.substitute_generics_and_consts_with_self(
                signature.return_type,
                &substitutions,
                &candidate.target_const_substitutions,
                candidate.self_ty,
            );
            let return_type = this.normalize_projection(return_type);
            Some(this.normalize_aliases_in_type(return_type))
        })
    }

    fn check_builtin_range_method(
        &mut self,
        call: &MethodCall<'_>,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let method = match *call.name {
            known::START => BuiltinMethod::Start,
            known::END => BuiltinMethod::End,
            _ => return None,
        };
        let mut self_ty = self.normalization.normalize(receiver_ty);
        let (kind, bound) = loop {
            match self.interner.get(self_ty).cloned()? {
                TyKind::Range {
                    kind,
                    bound: Some(bound),
                } => break (kind, bound),
                TyKind::Pointer { elem, .. } => {
                    self_ty = self.normalization.normalize(elem);
                }
                _ => return None,
            }
        };
        let has_bound = match method {
            BuiltinMethod::Start => kind.has_start_bound(),
            BuiltinMethod::End => kind.has_end_bound(),
            BuiltinMethod::SliceLen
            | BuiltinMethod::SlicePtr
            | BuiltinMethod::SlicePtrMut
            | BuiltinMethod::Iter => false,
        };
        if !has_bound {
            return None;
        }
        if call.type_args.is_some_and(|args| !args.is_empty()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                format!(
                    "range method `{}` does not accept generic arguments",
                    self.symbol_name(*call.name)
                ),
            ));
        }
        self.check_call_arg_count(call.span, call.args.len(), 0, false);
        for arg in call.args {
            self.check_expr(arg);
        }
        self.check_receiver_match(
            call.receiver,
            call.actual_receiver_ty,
            ReceiverKind::RefReadOnly,
        );
        if let Some(expected) = call.expected {
            self.expect_type(call.span, expected, bound, "range method call");
        }
        self.record_resolved_node_call(
            call.span,
            call.node_key,
            ResolvedCall::BuiltinMethod { method, self_ty },
        );
        Some(bound)
    }

    fn check_builtin_slice_pointer_method(
        &mut self,
        call: &MethodCall<'_>,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let (method, mutable) = match *call.name {
            known::PTR => (BuiltinMethod::SlicePtr, false),
            known::PTR_MUT => (BuiltinMethod::SlicePtrMut, true),
            _ => return None,
        };
        let self_ty = self.normalization.normalize(receiver_ty);
        let TyKind::Slice { is_readonly, elem } = self.interner.get(self_ty).cloned()? else {
            return None;
        };
        if mutable && is_readonly {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "slice method `ptrMut` requires a writable slice",
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if call.type_args.is_some_and(|args| !args.is_empty()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                format!(
                    "slice method `{}` does not accept generic arguments",
                    self.symbol_name(*call.name)
                ),
            ));
        }
        self.check_call_arg_count(call.span, call.args.len(), 0, false);
        for arg in call.args {
            self.check_expr(arg);
        }
        self.check_receiver_match(
            call.receiver,
            call.actual_receiver_ty,
            if mutable {
                ReceiverKind::Ref
            } else {
                ReceiverKind::RefReadOnly
            },
        );
        let output = self.interner.intern(TyKind::Pointer {
            is_readonly: !mutable,
            elem,
        });
        if let Some(expected) = call.expected {
            self.expect_type(call.span, expected, output, "slice pointer method call");
        }
        self.record_resolved_node_call(
            call.span,
            call.node_key,
            ResolvedCall::BuiltinMethod { method, self_ty },
        );
        Some(output)
    }

    fn dynamic_trait_object_receiver_ty(
        &mut self,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        match self.interner.get(receiver_ty).cloned() {
            Some(TyKind::TraitObject { .. }) => Some(receiver_ty),
            Some(TyKind::Pointer { elem, .. }) => {
                let elem = self.normalization.normalize(elem);
                matches!(self.interner.get(elem), Some(TyKind::TraitObject { .. })).then_some(elem)
            }
            _ => None,
        }
    }

    fn method_receiver_expr_expected_ty(
        &mut self,
        receiver_ty: InternedTyId,
        actual_ty: InternedTyId,
        receiver_kind: ReceiverKind,
    ) -> InternedTyId {
        match receiver_kind {
            ReceiverKind::Value => receiver_ty,
            ReceiverKind::RefReadOnly | ReceiverKind::Ref => {
                if self.receiver_expr_already_matches_receiver_ty(receiver_ty, actual_ty) {
                    return receiver_ty;
                }
                match self.interner.get(self.normalization.normalize(receiver_ty)) {
                    Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
                        *elem
                    }
                    _ => receiver_ty,
                }
            }
        }
    }

    fn receiver_expr_already_matches_receiver_ty(
        &mut self,
        receiver_ty: InternedTyId,
        actual_ty: InternedTyId,
    ) -> bool {
        if self.types_match(receiver_ty, actual_ty) {
            return true;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let actual_ty = self.normalization.normalize(actual_ty);
        match (self.interner.get(receiver_ty), self.interner.get(actual_ty)) {
            (
                Some(TyKind::Pointer {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                (*expected_readonly || !*actual_readonly)
                    && self.types_match(*expected_elem, *actual_elem)
            }
            _ => false,
        }
    }

    fn builtin_method_receiver_coercion(
        &mut self,
        receiver: &Expr,
        name: &SymbolId,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let trait_method = crate::symbols::builtin_trait_method_symbol(*name);
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
        let slice_is_readonly = match (trait_method, *name) {
            (Some(BuiltinTraitMethod::SliceMut), _) | (_, known::PTR_MUT) => {
                if is_readonly {
                    return None;
                }
                false
            }
            (Some(BuiltinTraitMethod::Slice), _) | (_, known::PTR) => true,
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
