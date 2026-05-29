// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind, ReceiverKind};
use nia_body_ir::{BracketSuffixResolution, ResolvedCall};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::FunctionSignature;
use nia_span::Span;
use nia_ty::{ArrayLenTy, TyKind};

struct MethodCall<'a> {
    span: Span,
    receiver: &'a Expr,
    receiver_ty: InternedTyId,
    name: &'a str,
    type_args: Option<&'a [BracketArg]>,
    args: &'a [Expr],
    expected: Option<InternedTyId>,
}

struct MethodGenericContext<'a> {
    span: Span,
    receiver_ty: InternedTyId,
    method_id: GlobalDefId,
    method_args: Option<&'a [BracketArg]>,
    lowered_method_args: &'a [InternedTyId],
    expected: Option<InternedTyId>,
}

#[derive(Clone, Copy)]
pub(super) struct MethodCandidate {
    pub(super) target_ty: InternedTyId,
    pub(super) method_id: GlobalDefId,
}

impl<'a> BodyChecker<'a> {
    pub(super) fn check_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.check_associated_call_inner(span, ty_expr, name, None, args, expected)
    }

    pub(super) fn check_explicit_generic_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        method_type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.check_associated_call_inner(
            span,
            ty_expr,
            name,
            Some(method_type_args),
            args,
            expected,
        )
    }

    fn check_associated_call_inner(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        method_type_args: Option<&[BracketArg]>,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let target_ty = self.associated_target_ty(ty_expr, expected, name)?;
        let candidates = self.method_candidates_for_target(target_ty, name);
        let Some(method_id) = self.single_method_candidate(span, name, candidates) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown associated function `{name}`"),
            ));
            return Some(self.error());
        };
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "associated function signature not found",
            ));
            return Some(self.error());
        };
        let mut substitutions = self.extension_target_substitutions(method_id, target_ty);
        let Some(method_instantiation_args) = self.lowered_method_type_args(method_type_args)
        else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let method_arg_count = method_instantiation_args.len();
        if method_type_args.is_some() && signature.generics.len() != method_arg_count {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "generic argument count mismatch for method: expected {}, got {method_arg_count}",
                    signature.generics.len()
                ),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if method_type_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(&signature.generics, &method_instantiation_args),
            );
        } else if let Some(expected) = expected {
            self.infer_generics_from_type(
                signature.return_type,
                expected,
                &mut substitutions,
                span,
            );
        }
        let is_receiver_method = signature
            .params
            .first()
            .is_some_and(|param| param.receiver.is_some());
        if is_receiver_method && args.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("receiver method `{name}` requires a receiver argument"),
            ));
            return Some(self.error());
        }
        if is_receiver_method
            && let Some(first_arg) = args.first()
            && let Some(receiver_kind) = signature.params.first().and_then(|param| param.receiver)
        {
            let receiver_ty = self.receiver_ty_for_target(target_ty, receiver_kind);
            let actual = self.check_expr_with_expected(first_arg, Some(receiver_ty));
            self.expect_expr_type(first_arg, receiver_ty, actual, "receiver argument");
        }
        let params: Vec<InternedTyId> = signature
            .params
            .iter()
            .skip(if is_receiver_method { 1 } else { 0 })
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        let value_args = if is_receiver_method && !args.is_empty() {
            &args[1..]
        } else {
            args
        };
        if method_type_args.is_none() {
            self.infer_method_generics_from_args(value_args, &params, &mut substitutions);
            if !self.method_generics_are_complete(span, &signature, &substitutions) {
                self.check_call_arg_count(span, value_args.len(), params.len(), false);
                return Some(self.error());
            }
        }
        let params: Vec<InternedTyId> = signature
            .params
            .iter()
            .skip(if is_receiver_method { 1 } else { 0 })
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        self.check_direct_call_args(span, value_args, &params, false);
        let target_args = self.extension_target_instance_args(method_id, &substitutions);
        if !target_args.is_empty() || !method_instantiation_args.is_empty() {
            let mut instance_args = target_args;
            instance_args.extend(method_instantiation_args);
            self.record_generic_instantiation(method_id, &instance_args, span);
            self.resolved_calls.insert(
                span,
                ResolvedCall::FunctionInstance {
                    def_id: method_id,
                    args: instance_args,
                },
            );
        } else {
            self.resolved_calls
                .insert(span, ResolvedCall::Function(method_id));
        }
        Some(self.substitute_generics(signature.return_type, &substitutions))
    }

    pub(super) fn receiver_ty_for_target(
        &mut self,
        target_ty: InternedTyId,
        receiver: ReceiverKind,
    ) -> InternedTyId {
        match receiver {
            ReceiverKind::Value => target_ty,
            ReceiverKind::RefConst => self.interner.intern(nia_ty::TyKind::Pointer {
                is_const: true,
                elem: target_ty,
            }),
            ReceiverKind::Ref => self.interner.intern(nia_ty::TyKind::Pointer {
                is_const: false,
                elem: target_ty,
            }),
        }
    }

    pub(super) fn associated_target_ty(
        &mut self,
        ty_expr: &Expr,
        expected: Option<InternedTyId>,
        name: &str,
    ) -> Option<InternedTyId> {
        if let ExprKind::TypeTarget { ty } = &ty_expr.kind {
            return Some(self.ty_for_span(ty.span));
        }
        let (struct_id, mut type_args) = self.type_prefix_instance(ty_expr)?;
        let candidates = self.method_candidates_for_struct(struct_id, name);
        if type_args.is_empty()
            && let Some(expected) = expected
            && let Some(inferred) =
                self.infer_associated_type_args_from_candidates(struct_id, &candidates, expected)
        {
            type_args = inferred;
        }
        if !type_args.is_empty() || self.type_prefix_has_no_generics(struct_id) {
            self.check_type_prefix_arg_count(ty_expr.span, struct_id, type_args.len());
            return Some(self.interner.intern(TyKind::Nominal {
                def_id: struct_id,
                args: type_args,
            }));
        }
        self.check_type_prefix_arg_count(ty_expr.span, struct_id, type_args.len());
        Some(self.interner.intern(TyKind::Nominal {
            def_id: struct_id,
            args: Vec::new(),
        }))
    }

    fn infer_associated_type_args_from_expected_return(
        &mut self,
        struct_id: GlobalDefId,
        signature: &FunctionSignature,
        expected: InternedTyId,
    ) -> Option<Vec<InternedTyId>> {
        let expected = self.normalization.normalize(expected);
        let Some(nia_ty::TyKind::Nominal {
            def_id: expected_def,
            args: expected_args,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        if expected_def != struct_id {
            return None;
        }
        let substitutions = self.struct_generic_substitutions(struct_id, &expected_args);
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        if self.types_match(expected, return_type) {
            Some(expected_args)
        } else {
            None
        }
    }

    fn infer_associated_type_args_from_candidates(
        &mut self,
        struct_id: GlobalDefId,
        candidates: &[MethodCandidate],
        expected: InternedTyId,
    ) -> Option<Vec<InternedTyId>> {
        let mut inferred = Vec::new();
        for candidate in candidates {
            let Some(signature) = self
                .resolved_function_signature(candidate.method_id)
                .map(|resolved| resolved.signature)
            else {
                continue;
            };
            if let Some(args) = self
                .infer_associated_type_args_from_expected_return(struct_id, &signature, expected)
                && !inferred.contains(&args)
            {
                inferred.push(args);
            }
        }
        match inferred.as_slice() {
            [args] => Some(args.clone()),
            _ => None,
        }
    }

    pub(super) fn type_prefix_has_no_generics(&mut self, def_id: GlobalDefId) -> bool {
        self.resolved_struct_signature(def_id)
            .map(|resolved| resolved.signature.generics.is_empty())
            .unwrap_or(false)
    }

    pub(super) fn check_type_prefix_arg_count(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        actual: usize,
    ) -> bool {
        let Some(signature) = self
            .resolved_struct_signature(def_id)
            .map(|resolved| resolved.signature)
        else {
            return true;
        };
        let expected = signature.generics.len();
        if expected == actual {
            return true;
        }
        let name = self
            .resolved_struct_signature(def_id)
            .map(|resolved| resolved.signature.span)
            .and_then(|_| {
                (def_id.module_id == self.defs.module_id)
                    .then(|| self.defs.defs.get(def_id.def_id))
                    .flatten()
            })
            .map(|def| def.name.as_str())
            .unwrap_or("<unknown>");
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "generic argument count mismatch for `{name}`: expected {expected}, got {actual}"
            ),
        ));
        false
    }

    pub(crate) fn type_prefix_def_id(&mut self, expr: &Expr) -> Option<GlobalDefId> {
        self.type_prefix_instance(expr).map(|(def_id, _)| def_id)
    }

    pub(crate) fn type_prefix_instance(
        &mut self,
        expr: &Expr,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if let ExprKind::BracketSuffix { callee, args } = &expr.kind {
            let def_id = self.type_prefix_def_id(callee)?;
            self.record_bracket_suffix_resolution(
                expr.span,
                BracketSuffixResolution::TypePrefixInstantiation,
            );
            let args = self.lower_bracket_type_args(args);
            return Some((def_id, args));
        }
        if let Some(ty) = self.type_uses.get(&expr.span).copied()
            && let Some(nia_ty::TyKind::Nominal { def_id, args }) = self.interner.get(ty)
        {
            return Some((*def_id, args.clone()));
        }
        if let ExprKind::Qualified { .. } = &expr.kind {
            if let Some(def_id) = self.values.qualified_type_prefixes.get(&expr.span).copied() {
                return Some((def_id, Vec::new()));
            }
            return None;
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return None;
        };
        if let Some(def_id) = self.values.qualified_type_prefixes.get(&expr.span).copied() {
            return Some((def_id, Vec::new()));
        }
        matches!(
            self.locals.uses.get(&expr.span),
            Some(nia_local_resolve::LocalUse::TypePrefix)
        )
        .then(|| {
            self.defs
                .module_scope
                .types
                .get(name)
                .map(|def_id| (self.global_def_id(def_id), Vec::new()))
        })?
    }

    pub(super) fn check_field_method_call(
        &mut self,
        span: Span,
        receiver: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.check_expr(receiver);
        let candidates = self.method_candidates_for_receiver(receiver_ty, name);
        if candidates.is_empty() {
            return None;
        }
        self.single_method_candidate(span, name, candidates)?;
        self.check_method_call_with_receiver_ty(MethodCall {
            span,
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
        span: Span,
        receiver: &Expr,
        name: &str,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.check_expr(receiver);
        let candidates = self.method_candidates_for_receiver(receiver_ty, name);
        if candidates.is_empty() {
            return None;
        }
        self.single_method_candidate(span, name, candidates)?;
        self.check_method_call_with_receiver_ty(MethodCall {
            span,
            receiver,
            receiver_ty,
            name,
            type_args: Some(type_args),
            args,
            expected,
        })
    }

    fn check_method_call_with_receiver_ty(&mut self, call: MethodCall<'_>) -> Option<InternedTyId> {
        let candidates = self.method_candidates_for_receiver(call.receiver_ty, call.name);
        let method_id = self.single_method_candidate(call.span, call.name, candidates)?;
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics
                .push(Diagnostic::error(call.span, "method signature not found"));
            return Some(self.error());
        };
        let Some(receiver_param) = signature
            .params
            .first()
            .filter(|param| param.receiver.is_some())
        else {
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "associated functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };

        let Some(receiver_kind) = receiver_param.receiver else {
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "internal compiler error: receiver method candidate has no receiver",
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
        let Some(mut substitutions) = self.method_generic_substitutions(
            MethodGenericContext {
                span: call.span,
                receiver_ty: call.receiver_ty,
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
            self.resolved_calls.insert(
                call.span,
                ResolvedCall::Method {
                    def_id: method_id,
                    args: instance_args,
                },
            );
        } else {
            self.resolved_calls.insert(
                call.span,
                ResolvedCall::Method {
                    def_id: method_id,
                    args: Vec::new(),
                },
            );
        }
        Some(self.substitute_generics(signature.return_type, &substitutions))
    }

    pub(super) fn method_candidates_for_struct(
        &mut self,
        struct_id: GlobalDefId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(target_ty, method_id)| {
                let target_ty = self.normalization.normalize(target_ty);
                matches!(
                    self.interner.get(target_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == struct_id
                )
                .then_some(MethodCandidate {
                    target_ty,
                    method_id,
                })
            })
            .collect()
    }

    pub(super) fn method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(candidate_ty, method_id)| {
                self.match_type_pattern(candidate_ty, target_ty, &mut HashMap::new())
                    .then_some(MethodCandidate {
                        target_ty: candidate_ty,
                        method_id,
                    })
            })
            .collect()
    }

    fn method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        let mut receiver_ty = self.normalization.normalize(receiver_ty);
        loop {
            let candidates = self
                .extensions
                .all_methods_named(name)
                .into_iter()
                .filter_map(|(target_ty, method_id)| {
                    let target_ty = self.normalization.normalize(target_ty);
                    self.match_type_pattern(target_ty, receiver_ty, &mut HashMap::new())
                        .then_some(MethodCandidate {
                            target_ty,
                            method_id,
                        })
                })
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return candidates;
            }
            match self.interner.get(receiver_ty) {
                Some(TyKind::Pointer { elem, .. }) => {
                    receiver_ty = self.normalization.normalize(*elem);
                }
                _ => return Vec::new(),
            }
        }
    }

    pub(super) fn single_method_candidate(
        &mut self,
        span: Span,
        name: &str,
        candidates: Vec<MethodCandidate>,
    ) -> Option<GlobalDefId> {
        let candidates = self.most_specific_candidates(&candidates);
        match candidates.as_slice() {
            [method] => Some(method.method_id),
            [] => None,
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("ambiguous method `{name}`"),
                ));
                None
            }
        }
    }

    fn most_specific_candidates(&self, candidates: &[MethodCandidate]) -> Vec<MethodCandidate> {
        candidates
            .iter()
            .copied()
            .filter(|candidate| {
                !candidates.iter().any(|other| {
                    other.method_id != candidate.method_id
                        && self.strictly_more_specific(other.target_ty, candidate.target_ty)
                })
            })
            .collect()
    }

    fn strictly_more_specific(&self, specific: InternedTyId, general: InternedTyId) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    fn pattern_subsumes(&self, general: InternedTyId, specific: InternedTyId) -> bool {
        self.pattern_subsumes_inner(general, specific, &mut HashMap::new())
    }

    fn pattern_subsumes_inner(
        &self,
        general: InternedTyId,
        specific: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let general = self.normalization.normalize(general);
        let specific = self.normalization.normalize(specific);
        match self.interner.get(general) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.patterns_equivalent(existing, specific)
                } else {
                    substitutions.insert(name.clone(), specific);
                    true
                }
            }
            Some(TyKind::Primitive(general_primitive)) => matches!(
                self.interner.get(specific),
                Some(TyKind::Primitive(specific_primitive)) if general_primitive == specific_primitive
            ),
            Some(TyKind::Pointer {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Pointer {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Slice {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Array {
                len: general_len,
                elem: general_elem,
            }) => match self.interner.get(specific) {
                Some(TyKind::Array {
                    len: specific_len,
                    elem: specific_elem,
                }) if self.array_lens_match(general_len, specific_len) => {
                    self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: general_params,
                return_type: general_return,
                is_variadic: general_variadic,
            }) => match self.interner.get(specific) {
                Some(TyKind::FunctionPointer {
                    params: specific_params,
                    return_type: specific_return,
                    is_variadic: specific_variadic,
                }) if general_variadic == specific_variadic
                    && general_params.len() == specific_params.len() =>
                {
                    general_params
                        .iter()
                        .zip(specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            *general_return,
                            *specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: general_def,
                args: general_args,
            }) => match self.interner.get(specific) {
                Some(TyKind::Nominal {
                    def_id: specific_def,
                    args: specific_args,
                }) if general_def == specific_def && general_args.len() == specific_args.len() => {
                    general_args
                        .iter()
                        .zip(specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::Error) | None => false,
        }
    }

    fn patterns_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.pattern_subsumes(left, right) && self.pattern_subsumes(right, left)
    }

    fn lowered_method_type_args(
        &mut self,
        type_args: Option<&[BracketArg]>,
    ) -> Option<Vec<InternedTyId>> {
        type_args
            .map(|args| self.lower_bracket_type_args(args))
            .or(Some(Vec::new()))
    }

    fn method_generic_substitutions(
        &mut self,
        context: MethodGenericContext<'_>,
        signature: &FunctionSignature,
    ) -> Option<HashMap<String, InternedTyId>> {
        let mut substitutions =
            self.extension_target_substitutions(context.method_id, context.receiver_ty);
        let method_arg_count = context.lowered_method_args.len();
        if context.method_args.is_some() && signature.generics.len() != method_arg_count {
            self.diagnostics.push(Diagnostic::error(
                context.span,
                format!(
                    "generic argument count mismatch for method: expected {}, got {method_arg_count}",
                    signature.generics.len()
                ),
            ));
            return None;
        }
        if context.method_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(&signature.generics, context.lowered_method_args),
            );
        } else if let Some(expected) = context.expected {
            self.infer_generics_from_type(
                signature.return_type,
                expected,
                &mut substitutions,
                context.span,
            );
        }
        Some(substitutions)
    }

    pub(super) fn extension_target_substitutions(
        &mut self,
        method_id: GlobalDefId,
        receiver_ty: InternedTyId,
    ) -> HashMap<String, InternedTyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return HashMap::new();
        };
        let mut substitutions = HashMap::new();
        self.match_receiver_target(target_ty, receiver_ty, &mut substitutions);
        substitutions
    }

    pub(super) fn extension_target_instance_args(
        &mut self,
        method_id: GlobalDefId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<InternedTyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return Vec::new();
        };
        self.generic_params_in_ty(target_ty)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn extension_target_ty_for_method(&self, method_id: GlobalDefId) -> Option<InternedTyId> {
        self.extensions
            .targets()
            .iter()
            .find(|target| {
                target
                    .methods
                    .iter()
                    .any(|method| method.def_id == method_id)
            })
            .map(|target| target.target_ty)
    }

    fn match_receiver_target(
        &self,
        target_ty: InternedTyId,
        receiver_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        if self.match_type_pattern(target_ty, receiver_ty, substitutions) {
            return true;
        }
        match self.interner.get(receiver_ty) {
            Some(TyKind::Pointer { elem, .. }) => {
                self.match_receiver_target(target_ty, *elem, substitutions)
            }
            _ => false,
        }
    }

    pub(super) fn match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        match self.interner.get(pattern) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Pointer {
                    is_const,
                    elem
                }) if is_const == pattern_const
                    && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Slice {
                    is_const,
                    elem
                }) if is_const == pattern_const
                    && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.interner.get(actual) {
                Some(TyKind::Array { len, elem }) if self.array_lens_match(pattern_len, len) => {
                    self.match_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: pattern_params,
                return_type: pattern_return,
                is_variadic: pattern_variadic,
            }) => match self.interner.get(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_type_pattern(*pattern_return, *return_type, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.interner.get(actual) {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    fn array_lens_match(&self, expected: &ArrayLenTy, actual: &ArrayLenTy) -> bool {
        if expected == actual {
            return true;
        }
        let expected = self.array_len_value(Span::default(), expected).ok();
        let actual = self.array_len_value(Span::default(), actual).ok();
        expected.is_some() && expected == actual
    }

    fn infer_method_generics_from_args(
        &mut self,
        args: &[Expr],
        params: &[InternedTyId],
        substitutions: &mut HashMap<String, InternedTyId>,
    ) {
        let actuals = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if let Some(expected) = params
                    .get(index)
                    .copied()
                    .map(|param| self.substitute_generics(param, substitutions))
                {
                    self.check_expr_with_expected(arg, Some(expected))
                } else {
                    self.check_expr(arg)
                }
            })
            .collect::<Vec<_>>();
        for (param, (arg, actual)) in params.iter().zip(args.iter().zip(actuals.iter())) {
            self.infer_generics_from_type(*param, *actual, substitutions, arg.span);
        }
    }

    fn method_generics_are_complete(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        let mut complete = true;
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                complete = false;
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("cannot infer generic parameter `{generic}`"),
                ));
            }
        }
        complete
    }

    fn check_receiver_match(
        &mut self,
        receiver: &Expr,
        receiver_ty: InternedTyId,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind == ReceiverKind::Ref {
            let base = self.receiver_base_type(receiver_ty);
            if base.as_ref().is_some_and(|base| base.has_readonly_pointer) {
                self.diagnostics.push(Diagnostic::error(
                    receiver.span,
                    "receiver cannot be matched through `&const T`",
                ));
            } else if !base.as_ref().is_some_and(|base| base.from_pointer) {
                self.check_assignable(receiver, "receiver");
            }
        }
    }
}
