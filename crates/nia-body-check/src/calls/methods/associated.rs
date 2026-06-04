// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(in crate::calls) fn check_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.check_associated_call_inner(span, ty_expr, name, None, args, expected)
    }

    pub(in crate::calls) fn check_explicit_generic_associated_call(
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
        if candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_trait_associated_method_call(
                span,
                target_ty,
                name,
                method_type_args,
                args,
                expected,
            )
        {
            return Some(return_ty);
        }
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
            self.record_resolved_call(
                span,
                ResolvedCall::FunctionInstance {
                    def_id: method_id,
                    args: instance_args,
                },
            );
        } else {
            self.record_resolved_call(span, ResolvedCall::Function(method_id));
        }
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        Some(self.normalize_projection(return_type))
    }

    pub(in crate::calls) fn receiver_ty_for_target(
        &mut self,
        target_ty: InternedTyId,
        receiver: ReceiverKind,
    ) -> InternedTyId {
        match receiver {
            ReceiverKind::Value => target_ty,
            ReceiverKind::RefReadOnly => self.interner.intern(nia_ty::TyKind::Pointer {
                is_readonly: true,
                elem: target_ty,
            }),
            ReceiverKind::Ref => self.interner.intern(nia_ty::TyKind::Pointer {
                is_readonly: false,
                elem: target_ty,
            }),
        }
    }

    pub(in crate::calls::methods) fn associated_target_ty(
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

    pub(in crate::calls) fn type_prefix_has_no_generics(&mut self, def_id: GlobalDefId) -> bool {
        self.resolved_struct_signature(def_id)
            .map(|resolved| resolved.signature.generics.is_empty())
            .unwrap_or(false)
    }

    pub(in crate::calls) fn check_type_prefix_arg_count(
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
}
