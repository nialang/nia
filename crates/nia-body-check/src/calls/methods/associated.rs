// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct TraitAssociatedFunctionCall<'b> {
    expr: &'b Expr,
    target_ty: InternedTyId,
    name: &'b str,
    method_type_args: Option<&'b [BracketArg]>,
    args: &'b [Expr],
    expected: Option<InternedTyId>,
    candidates: Vec<TraitMethodCandidate>,
}

impl<'a> BodyChecker<'a> {
    pub(in crate::calls) fn check_associated_call(
        &mut self,
        expr: &Expr,
        ty_expr: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.check_associated_call_inner(expr, ty_expr, name, None, args, expected)
    }

    pub(in crate::calls) fn check_explicit_generic_associated_call(
        &mut self,
        expr: &Expr,
        ty_expr: &Expr,
        name: &str,
        method_type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self.check_associated_call_inner(
            expr,
            ty_expr,
            name,
            Some(method_type_args),
            args,
            expected,
        )
    }

    fn check_associated_call_inner(
        &mut self,
        expr: &Expr,
        ty_expr: &Expr,
        name: &str,
        method_type_args: Option<&[BracketArg]>,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let target_ty = self.associated_target_ty(ty_expr, expected, name)?;
        self.check_associated_call_for_target(
            expr,
            target_ty,
            name,
            method_type_args,
            args,
            expected,
        )
    }

    pub(in crate::calls) fn check_associated_call_for_target(
        &mut self,
        expr: &Expr,
        target_ty: InternedTyId,
        name: &str,
        method_type_args: Option<&[BracketArg]>,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let span = expr.span;
        let candidates = self.method_candidates_for_target(target_ty, name);
        let trait_candidates = if candidates.is_empty() {
            self.trait_method_candidates_for_target(target_ty, name)
        } else {
            Vec::new()
        };
        if candidates.is_empty()
            && trait_candidates.is_empty()
            && let Some(return_ty) = self.check_builtin_trait_associated_method_call(
                expr,
                target_ty,
                name,
                method_type_args,
                args,
                expected,
            )
        {
            return Some(return_ty);
        }
        if candidates.is_empty() && !trait_candidates.is_empty() {
            return self.check_trait_associated_function_call(TraitAssociatedFunctionCall {
                expr,
                target_ty,
                name,
                method_type_args,
                args,
                expected,
                candidates: trait_candidates,
            });
        }
        let Some(method_id) = self.single_method_candidate(span, name, &candidates) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                format!("unknown associated function `{name}`"),
            ));
            return Some(self.error());
        };
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                "associated function signature not found",
            ));
            return Some(self.error());
        };
        let Some(mut substitutions) = self.extension_target_substitutions(method_id, target_ty)
        else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let Some(method_instantiation_args) = self.lowered_method_type_args(method_type_args)
        else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let method_arg_count = method_instantiation_args.len();
        if method_type_args.is_some() && signature.generics.len() != method_arg_count {
            self.diagnostics.push(Diagnostic::user_error_at("E0301", 
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
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
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
            self.record_resolved_node_call(
                span,
                &expr.node_key,
                ResolvedCall::FunctionInstance {
                    def_id: method_id,
                    arg_module_id: self.defs.module_id,
                    args: instance_args,
                },
            );
        } else {
            self.record_resolved_node_call(span, &expr.node_key, ResolvedCall::Function(method_id));
        }
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        let return_type = self.normalize_projection(return_type);
        Some(self.normalize_aliases_in_type(return_type))
    }

    fn check_trait_associated_function_call(
        &mut self,
        call: TraitAssociatedFunctionCall<'_>,
    ) -> Option<InternedTyId> {
        let TraitAssociatedFunctionCall {
            expr,
            target_ty,
            name,
            method_type_args,
            args,
            expected,
            candidates,
        } = call;
        let candidates = candidates
            .into_iter()
            .filter(|candidate| {
                self.trait_associated_candidate_matches_args(candidate, target_ty, args)
            })
            .collect::<Vec<_>>();
        let candidate = match candidates.as_slice() {
            [candidate] => candidate,
            [] => return None,
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    expr.span,
                    format!("ambiguous trait associated function `{name}`"),
                ));
                return Some(self.error());
            }
        };
        if candidate
            .signature
            .params
            .first()
            .is_some_and(|param| param.receiver.is_some())
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                expr.span,
                format!("receiver method `{name}` requires a receiver argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let Some(method_instantiation_args) = self.lowered_method_type_args(method_type_args)
        else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        if method_type_args.is_some()
            && candidate.signature.generics.len() != method_instantiation_args.len()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                expr.span,
                format!(
                    "generic argument count mismatch for trait method: expected {}, got {}",
                    candidate.signature.generics.len(),
                    method_instantiation_args.len()
                ),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let mut substitutions =
            self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
        substitutions.insert("Self".to_string(), target_ty);
        if method_type_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(
                    &candidate.signature.generics,
                    &method_instantiation_args,
                ),
            );
        } else if let Some(expected) = expected {
            let return_type =
                self.substitute_generics(candidate.signature.return_type, &substitutions);
            let expected = self.normalize_projection(expected);
            self.infer_generics_from_type(return_type, expected, &mut substitutions, expr.span);
        }
        let mut params: Vec<InternedTyId> = candidate
            .signature
            .params
            .iter()
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        if method_type_args.is_none() {
            self.infer_method_generics_from_args(args, &params, &mut substitutions);
            if !self.method_generics_are_complete(expr.span, &candidate.signature, &substitutions) {
                self.check_call_arg_count(expr.span, args.len(), params.len(), false);
                return Some(self.error());
            }
            params = candidate
                .signature
                .params
                .iter()
                .map(|param| self.substitute_generics(param.ty, &substitutions))
                .collect();
        }
        self.check_direct_call_args(expr.span, args, &params, false);
        let trait_args = candidate
            .trait_args
            .iter()
            .map(|arg| self.substitute_generics(*arg, &substitutions))
            .collect::<Vec<_>>();
        if candidate.has_default {
            let mut instance_args = vec![target_ty];
            instance_args.extend(trait_args.iter().copied());
            instance_args.extend(method_instantiation_args.iter().copied());
            self.record_generic_instantiation(candidate.method_id, &instance_args, expr.span);
        }
        self.record_resolved_node_call(
            expr.span,
            &expr.node_key,
            ResolvedCall::TraitAssociatedFunction {
                trait_id: candidate.trait_id,
                method_id: candidate.method_id,
                method_name: name.to_string(),
                self_ty: target_ty,
                trait_args,
                args: method_instantiation_args,
            },
        );
        let return_type = self.substitute_generics(candidate.signature.return_type, &substitutions);
        let return_type = self.normalize_projection(return_type);
        Some(self.normalize_aliases_in_type(return_type))
    }

    fn trait_associated_candidate_matches_args(
        &mut self,
        candidate: &TraitMethodCandidate,
        target_ty: InternedTyId,
        args: &[Expr],
    ) -> bool {
        let mut substitutions =
            self.generic_substitutions(&candidate.trait_generics, &candidate.trait_args);
        substitutions.insert("Self".to_string(), target_ty);
        for (index, arg) in args.iter().enumerate() {
            let Some(param) = candidate.signature.params.get(index) else {
                continue;
            };
            let expected = self.substitute_generics(param.ty, &substitutions);
            if self.type_contains_generic_param(expected) {
                continue;
            }
            if !self.expr_can_match_expected(arg, expected) {
                return false;
            }
        }
        true
    }

    pub(in crate::calls) fn associated_target_ty(
        &mut self,
        ty_expr: &Expr,
        expected: Option<InternedTyId>,
        name: &str,
    ) -> Option<InternedTyId> {
        if let ExprKind::TypeTarget { ty } = &ty_expr.kind {
            let ty = self.ty_for_type(ty);
            return Some(self.normalization.normalize(ty));
        }
        let (nominal_def_id, mut type_args) = self.type_prefix_instance(ty_expr)?;
        if type_args.is_empty()
            && let Some(expected) = expected
            && let Some(prefix_ty) = self.associated_nominal_generic_target_ty(nominal_def_id)
            && let Some(nia_ty::TyKind::Nominal {
                def_id: normalized_def_id,
                ..
            }) = self.interner.get(prefix_ty).cloned()
            && let candidates = self.method_candidates_for_target(prefix_ty, name)
            && let Some(inferred) = self.infer_associated_type_args_from_candidates(
                normalized_def_id,
                &candidates,
                expected,
            )
        {
            type_args = inferred;
        }
        if !type_args.is_empty() || self.nominal_type_prefix_has_no_generics(nominal_def_id) {
            self.check_type_prefix_arg_count(ty_expr.span, nominal_def_id, type_args.len());
            return self.associated_nominal_target_ty(nominal_def_id, type_args);
        }
        self.check_type_prefix_arg_count(ty_expr.span, nominal_def_id, type_args.len());
        self.associated_nominal_target_ty(nominal_def_id, Vec::new())
    }

    fn associated_nominal_target_ty(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    ) -> Option<InternedTyId> {
        if let Some(target) = self.expand_type_alias_instance(def_id, &args) {
            return Some(target);
        }
        let ty = self.interner.intern(TyKind::Nominal { def_id, args });
        Some(self.normalization.normalize(ty))
    }

    fn associated_nominal_generic_target_ty(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        let args = self
            .nominal_type_generics(def_id)?
            .into_iter()
            .map(|generic| self.interner.intern(TyKind::GenericParam(generic)))
            .collect();
        self.associated_nominal_target_ty(def_id, args)
    }

    fn infer_associated_type_args_from_expected_return(
        &mut self,
        nominal_def_id: GlobalDefId,
        signature: &FunctionSignature,
        expected: InternedTyId,
    ) -> Option<Vec<InternedTyId>> {
        let expected = self.associated_expected_return_ty(expected);
        let Some(nia_ty::TyKind::Nominal {
            def_id: expected_def,
            args: expected_args,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        if expected_def != nominal_def_id {
            return None;
        }
        let substitutions = self.nominal_type_generic_substitutions(nominal_def_id, &expected_args);
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        let return_type = self.normalize_aliases_in_type(return_type);
        if self.types_match(expected, return_type) {
            Some(expected_args)
        } else {
            None
        }
    }

    fn associated_expected_return_ty(&mut self, expected: InternedTyId) -> InternedTyId {
        let expected = self.normalization.normalize(expected);
        match self.interner.get(expected).cloned() {
            Some(TyKind::Pointer { elem, .. }) => {
                let elem = self.normalization.normalize(elem);
                match self.interner.get(elem).cloned() {
                    Some(TyKind::FunctionPointer { return_type, .. }) => {
                        self.normalization.normalize(return_type)
                    }
                    _ => expected,
                }
            }
            Some(TyKind::FunctionPointer { return_type, .. }) => {
                self.normalization.normalize(return_type)
            }
            _ => expected,
        }
    }

    fn infer_associated_type_args_from_candidates(
        &mut self,
        nominal_def_id: GlobalDefId,
        candidates: &[MethodCandidate],
        expected: InternedTyId,
    ) -> Option<Vec<InternedTyId>> {
        let mut inferred = Vec::new();
        for candidate in candidates {
            let Some(signature) = self
                .resolved_function_signature(candidate.method.def_id)
                .map(|resolved| resolved.signature)
            else {
                continue;
            };
            if let Some(args) = self.infer_associated_type_args_from_expected_return(
                nominal_def_id,
                &signature,
                expected,
            ) && !inferred.contains(&args)
            {
                inferred.push(args);
            }
        }
        match inferred.as_slice() {
            [args] => Some(args.clone()),
            _ => None,
        }
    }

    pub(in crate::calls) fn nominal_type_prefix_has_no_generics(
        &mut self,
        def_id: GlobalDefId,
    ) -> bool {
        self.nominal_type_generics(def_id)
            .map(|generics| generics.is_empty())
            .unwrap_or(false)
    }

    pub(in crate::calls) fn check_type_prefix_arg_count(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        actual: usize,
    ) -> bool {
        let Some(generics) = self.nominal_type_generics(def_id) else {
            return true;
        };
        let expected = generics.len();
        if expected == actual {
            return true;
        }
        let name = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .map(|def| def.name.as_str())
            .unwrap_or("<unknown>");
        self.diagnostics.push(Diagnostic::user_error_at(
            "E0301",
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
            self.record_bracket_suffix_node_resolution(
                expr,
                BracketSuffixResolution::TypePrefixInstantiation,
            );
            let args = self.lower_bracket_type_args(args);
            return Some((def_id, args));
        }
        if let ExprKind::TypeTarget { ty } = &expr.kind
            && let Some(ty) = self.node_type_uses.get(&ty.node_key).copied()
            && let Some(nia_ty::TyKind::Nominal { def_id, args }) = self.interner.get(ty)
        {
            return Some((*def_id, args.clone()));
        }
        if let ExprKind::Qualified { .. } = &expr.kind {
            if let Some(def_id) = self.qualified_type_prefix(expr) {
                return Some((def_id, Vec::new()));
            }
            return None;
        }
        let ExprKind::Ident(name) = &expr.kind else {
            return None;
        };
        if let Some(def_id) = self.qualified_type_prefix(expr) {
            return Some((def_id, Vec::new()));
        }
        matches!(
            self.local_use(expr),
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
