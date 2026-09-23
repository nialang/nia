// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_defs::VisibleExtensionMethod;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, ReceiverKind, TraitId, Visibility};
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

enum ContextualReceiverInference {
    Unavailable,
    Unique(InternedTyId),
    Ambiguous,
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
    pub(super) inaccessible_visibility: Option<Visibility>,
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
    fn check_contextual_receiver_expr(
        &mut self,
        receiver: &Expr,
        inference: ContextualReceiverInference,
    ) -> Option<InternedTyId> {
        self.profile_stage(
            "body_check.profile.method.receiver_expr",
            |this| match inference {
                ContextualReceiverInference::Unique(expected) => {
                    Some(this.check_expr_with_expected(receiver, Some(expected)))
                }
                ContextualReceiverInference::Unavailable => Some(this.check_expr(receiver)),
                ContextualReceiverInference::Ambiguous => None,
            },
        )
    }

    pub(super) fn check_field_method_call(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &SymbolId,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let receiver_expected = match expected {
            Some(expected) => {
                self.contextual_method_receiver_type(expr, receiver, name, None, args, expected)
            }
            None => ContextualReceiverInference::Unavailable,
        };
        let Some(receiver_ty) = self.check_contextual_receiver_expr(receiver, receiver_expected)
        else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("ambiguous method `{name}` while inferring receiver type"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
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
        let receiver_expected = match expected {
            Some(expected) => self.contextual_method_receiver_type(
                expr,
                receiver,
                name,
                Some(type_args),
                args,
                expected,
            ),
            None => ContextualReceiverInference::Unavailable,
        };
        let Some(receiver_ty) = self.check_contextual_receiver_expr(receiver, receiver_expected)
        else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("ambiguous method `{name}` while inferring receiver type"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
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
        let (inaccessible_candidates, candidates): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|candidate| candidate.inaccessible_visibility.is_some());
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
        if viable_candidates.is_empty()
            && candidates.is_empty()
            && !inaccessible_candidates.is_empty()
        {
            self.report_inaccessible_extension_method(
                call.span,
                call.name,
                &inaccessible_candidates,
            );
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if viable_candidates.is_empty() && candidates.len() > 1 {
            let name = self.symbol_name(*call.name);
            self.report_method_candidates(
                call.span,
                format!("no matching method overload `{name}`"),
                "none of the available method signatures accepts these arguments",
                "change the receiver or argument types to match one of the candidates, or call the method with explicit generic arguments",
                receiver_ty,
                &candidates,
            );
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if viable_candidates.is_empty()
            && candidates.is_empty()
            && receiver_ty != self.error()
            && !matches!(self.interner.get(receiver_ty), Some(TyKind::ConstOnly))
            && !self.supports_field_access(receiver_ty)
        {
            self.record_method_provider_demand(receiver_ty, *call.name);
            let name = self.symbol_name(*call.name);
            let receiver_name = self.ty_name(receiver_ty);
            let summary = format!("unknown method `{name}`");
            self.diagnostics.push(
                Diagnostic::user_error(codes::NAME_RESOLUTION, summary.clone())
                    .primary(call.span, format!("no method named `{name}` is available here"))
                    .note(format!("the receiver has type `{receiver_name}`"))
                    .help(format!(
                        "check the method name, import a trait that provides `{name}`, or use a field access"
                    ))
                    .finish(),
            );
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if viable_candidates.is_empty() {
            self.record_method_provider_demand(receiver_ty, *call.name);
        }
        let candidate = self.profile_stage("body_check.profile.method.single_candidate", |this| {
            this.single_method_candidate(call.span, call.name, &viable_candidates)
        });
        let Some(candidate) = candidate else {
            return (!viable_candidates.is_empty()).then_some(self.error());
        };
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
        let Some(receiver_kind) = signature.params.first().and_then(|param| param.receiver) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call.span,
                "associated functions are not supported by receiver method call syntax",
            ));
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

        let Some((method_instantiation_args, method_const_args)) =
            self.profile_stage("body_check.profile.method.lower_type_args", |this| {
                this.lowered_method_type_args(call.span, call.type_args, &signature.generic_params)
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

    pub(in crate::calls) fn report_inaccessible_extension_method(
        &mut self,
        span: Span,
        method_name: &SymbolId,
        candidates: &[MethodCandidate],
    ) {
        let Some(candidate) = candidates
            .iter()
            .min_by_key(|candidate| candidate.method.def_id)
        else {
            return;
        };
        let visibility = candidate
            .inaccessible_visibility
            .expect("candidate is inaccessible");
        let name = self.symbol_name(*method_name);
        let (summary, help) = match visibility {
            Visibility::Private => (
                format!("method `{name}` is private"),
                "make the method public or use it from an allowed scope".to_string(),
            ),
            Visibility::PublicSuper => (
                format!("method `{name}` is restricted to its parent module and descendants"),
                "make the method public or move this use into its permitted scope".to_string(),
            ),
            Visibility::PublicPkg => (
                format!("method `{name}` is restricted to its package"),
                "make the method public or use it from the defining package".to_string(),
            ),
            Visibility::Public => (
                format!("method `{name}` is not visible from this module"),
                "check the module path and declaration visibility".to_string(),
            ),
        };
        let method_id = candidate.method.def_id;
        let mut diagnostic = Diagnostic::user_error(codes::NAME_RESOLUTION, summary)
            .primary(span, "this method is not visible from this module");
        let declaration_span = self
            .program
            .defs
            .and_then(|defs| defs(method_id.module_id))
            .and_then(|defs| defs.defs.get(method_id.def_id).map(|def| def.span));
        let source_path = self
            .program
            .module_source_path
            .and_then(|source_path| source_path(method_id.module_id));
        if let (Some(path), Some(span)) = (source_path, declaration_span) {
            diagnostic = diagnostic.related_at(
                path.as_str(),
                span,
                "the restricted method is declared here",
            );
        }
        self.diagnostics.push(diagnostic.help(help).finish());
    }

    pub(in crate::calls) fn report_method_candidates(
        &mut self,
        span: Span,
        summary: String,
        note: &str,
        help: &str,
        receiver_ty: InternedTyId,
        candidates: &[MethodCandidate],
    ) {
        let receiver_name = self.ty_name(receiver_ty);
        let mut signatures = candidates
            .iter()
            .take(8)
            .filter_map(|candidate| self.method_candidate_signature(candidate))
            .collect::<Vec<_>>();
        signatures.sort();
        signatures.dedup();
        let mut diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
            .primary(span, summary)
            .note(format!("the receiver has type `{receiver_name}`; {note}"))
            .help(help);
        if !signatures.is_empty() {
            let list = signatures
                .iter()
                .map(|signature| format!("  - {signature}"))
                .collect::<Vec<_>>()
                .join("\n");
            diagnostic = diagnostic.note(format!("candidate methods:\n{list}"));
        }
        if candidates.len() > signatures.len() {
            diagnostic = diagnostic.note(format!(
                "{} additional candidate(s) omitted",
                candidates.len().saturating_sub(signatures.len())
            ));
        }
        self.diagnostics.push(diagnostic.finish());
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
                TyKind::Range { kind, bound } => break (kind, bound),
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
            let name = self.symbol_name(*call.name);
            let (boundary, article) = match method {
                BuiltinMethod::Start => ("start", "a"),
                BuiltinMethod::End => ("end", "an"),
                _ => return None,
            };
            let summary = format!(
                "range method `{name}` is unavailable because this range has no {boundary} bound"
            );
            self.diagnostics.push(
                Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                    .primary(call.span, summary)
                    .note(format!(
                        "the receiver has type `{}`",
                        self.ty_name(receiver_ty)
                    ))
                    .help(format!(
                        "use `{name}()` only on ranges with {article} {boundary} bound"
                    ))
                    .finish(),
            );
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let bound = bound?;
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
