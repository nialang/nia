// SPDX-License-Identifier: GPL-3.0-or-later
use super::ResolvedFunctionSignature;
use super::builtins::BuiltinCallTypeArgs;
use super::std_builtin_function;
use crate::{BodyChecker, generic_inst_base};
use nia_ast::{BracketArg, Expr, ExprKind, UnaryOp};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinFunction, GlobalDefId, InternedTyId};
use nia_item_signatures::{FunctionAttribute, FunctionSignature, GenericParamSignatureKind};
use nia_local_resolve::LocalUse;
use nia_sema_ir::{BracketSuffixResolution, FunctionReference, ResolvedCall};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, ConstGenericArg, ConstGenericValue, IntConst, PrimitiveTy,
    TyKind,
};
use nia_value_resolve::ValueNameResolution;

struct FunctionItemRef {
    resolved: ResolvedFunctionSignature,
    type_args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
    receiver_ty: Option<InternedTyId>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn direct_callee_signature(
        &mut self,
        callee: &Expr,
    ) -> Option<ResolvedFunctionSignature> {
        let base = generic_inst_base(callee);
        let ExprKind::Ident(_) = &base.kind else {
            return None;
        };
        let Some(ValueNameResolution::Def(def_id)) = self.value_name(base) else {
            return None;
        };
        self.signatures
            .functions
            .get(&def_id)
            .cloned()
            .map(|signature| ResolvedFunctionSignature {
                def_id: self.global_def_id(def_id),
                signature: self.local_function_signature(&signature),
            })
    }

    fn current_extension_method_callee_signature(
        &mut self,
        callee: &Expr,
    ) -> Option<FunctionItemRef> {
        let base = generic_inst_base(callee);
        let ExprKind::Ident(name) = &base.kind else {
            return None;
        };
        if matches!(
            self.value_name(base),
            Some(
                ValueNameResolution::Def(_)
                    | ValueNameResolution::External(_)
                    | ValueNameResolution::Module
            )
        ) {
            return None;
        }
        if !matches!(self.local_use(base), None | Some(LocalUse::Unresolved)) {
            return None;
        }
        let current_def_id = self.current_def_id?;
        let target_ty = self
            .ensure_extension_method_lookup_for_id(current_def_id)?
            .target_ty;
        let candidates = self.method_candidates_for_target(target_ty, name);
        let candidate = self.single_method_candidate(callee.span, name, &candidates)?;
        let method_id = candidate.method.def_id;
        let resolved = self.resolved_function_signature(method_id)?;
        let receiver_ty = resolved
            .signature
            .params
            .first()
            .and_then(|param| param.receiver)
            .map(|receiver| self.receiver_ty_for_target(target_ty, receiver));
        let (type_args, const_args) = self.extension_target_instance_args(
            method_id,
            &candidate.target_substitutions,
            &candidate.target_const_substitutions,
        );
        Some(FunctionItemRef {
            resolved,
            type_args,
            const_args,
            receiver_ty,
        })
    }

    pub(crate) fn check_function_ref(
        &mut self,
        expr: &Expr,
        is_readonly: bool,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let item = self.function_item_ref(expr, expected)?;
        self.reject_const_operation(
            expr.span,
            "function pointer values are not available during const evaluation",
        );
        if !is_readonly {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "function pointers must be formed with `&fn(...)`",
            ));
            return Some(self.error());
        }
        let signature = item.resolved.signature;
        if let Some(builtin) = builtin_function(&signature) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "builtin function `{}` cannot be used as a function pointer",
                    builtin.name()
                ),
            ));
            return Some(self.error());
        }
        let (substitutions, const_substitutions) = self.generic_substitutions_for_function_ref(
            expr,
            item.resolved.def_id,
            &signature,
            &item.type_args,
            &item.const_args,
        )?;
        let params = signature
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                if index == 0
                    && param.receiver.is_some()
                    && let Some(receiver_ty) = item.receiver_ty
                {
                    receiver_ty
                } else {
                    self.substitute_generics_and_consts(
                        param.ty,
                        &substitutions,
                        &const_substitutions,
                    )
                }
            })
            .collect();
        let return_type = self.substitute_generics_and_consts(
            signature.return_type,
            &substitutions,
            &const_substitutions,
        );
        let return_type = self.normalize_projection(return_type);
        let return_type = self.normalize_aliases_in_type(return_type);
        Some(self.interner.intern(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic: signature.is_variadic,
        }))
    }

    fn function_item_ref(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<FunctionItemRef> {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => {
                let mut item = self.function_item_ref(callee, expected)?;
                self.record_bracket_suffix_node_resolution(
                    expr,
                    BracketSuffixResolution::GenericCall,
                );
                if item.type_args.is_empty() && item.const_args.is_empty() {
                    let lowered = self.lower_bracket_args_for_generic_params(
                        expr.span,
                        &item.resolved.signature.generic_params,
                        args,
                    )?;
                    item.type_args.extend(lowered.type_args);
                    item.const_args.extend(lowered.const_args);
                } else {
                    let type_args = self.lower_bracket_type_args(args);
                    item.type_args.extend(type_args);
                }
                Some(item)
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(item) = self.associated_method_item_ref(expr.span, lhs, name, expected)
                {
                    return Some(item);
                }
                self.qualified_callee_signature(expr)
                    .map(|resolved| FunctionItemRef {
                        resolved,
                        type_args: Vec::new(),
                        const_args: Vec::new(),
                        receiver_ty: None,
                    })
            }
            _ => self
                .qualified_callee_signature(expr)
                .or_else(|| self.direct_callee_signature(expr))
                .map(|resolved| FunctionItemRef {
                    resolved,
                    type_args: Vec::new(),
                    const_args: Vec::new(),
                    receiver_ty: None,
                })
                .or_else(|| self.current_extension_method_callee_signature(expr)),
        }
    }

    fn associated_method_item_ref(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &SymbolId,
        expected: Option<InternedTyId>,
    ) -> Option<FunctionItemRef> {
        let target_ty = self.associated_target_ty(ty_expr, expected, name)?;
        let candidates = self.method_candidates_for_target(target_ty, name);
        let candidate = self.single_method_candidate(span, name, &candidates)?;
        let method_id = candidate.method.def_id;
        let resolved = self.resolved_function_signature(method_id)?;
        let receiver_ty = resolved
            .signature
            .params
            .first()
            .and_then(|param| param.receiver)
            .map(|receiver| self.receiver_ty_for_target(target_ty, receiver));
        let (type_args, const_args) = self.extension_target_instance_args(
            method_id,
            &candidate.target_substitutions,
            &candidate.target_const_substitutions,
        );
        Some(FunctionItemRef {
            resolved,
            type_args,
            const_args,
            receiver_ty,
        })
    }

    fn generic_substitutions_for_function_ref(
        &mut self,
        expr: &Expr,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<(SymbolMap<InternedTyId>, SymbolMap<ConstGenericArg>)> {
        let span = expr.span;
        let effective_generics = self.effective_generics_for_def(def_id);
        let effective_const_generics = self.effective_const_generics_for_def(def_id, signature);
        let type_generics: Vec<_> = effective_generics
            .iter()
            .copied()
            .filter(|name| !effective_const_generics.contains(name))
            .collect();
        let const_generics: Vec<_> = effective_generics
            .iter()
            .copied()
            .filter(|name| effective_const_generics.contains(name))
            .collect();
        if type_generics.len() != type_args.len() || const_generics.len() != const_args.len() {
            let message = if type_args.is_empty() && const_args.is_empty() {
                "generic function pointer requires explicit type arguments".to_string()
            } else {
                format!(
                    "generic argument count mismatch for function pointer: expected {} type and {} const arguments, got {} type and {} const arguments",
                    type_generics.len(),
                    const_generics.len(),
                    type_args.len(),
                    const_args.len()
                )
            };
            self.diagnostics
                .push(Diagnostic::user_error_at(codes::TYPE_CHECK, span, message));
            return None;
        }
        if !type_args.is_empty() || !const_args.is_empty() {
            self.record_generic_instantiation_with_const_args(def_id, type_args, const_args, span);
        }
        self.record_function_node_reference(
            span,
            &expr.node_key,
            FunctionReference {
                def_id,
                arg_module_id: self.defs.module_id,
                args: type_args.to_vec(),
                const_args: const_args.to_vec(),
            },
        );
        let mut substitutions = self.generic_substitutions(&type_generics, type_args);
        let mut const_substitutions = SymbolMap::default();
        for (name, arg) in const_generics
            .iter()
            .copied()
            .zip(const_args.iter().cloned())
        {
            const_substitutions.insert(name, arg);
        }
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) && !const_substitutions.contains_key(generic) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "generic function pointer requires `{}`",
                        self.symbol_name(*generic)
                    ),
                ));
                return None;
            }
        }
        Some((std::mem::take(&mut substitutions), const_substitutions))
    }

    pub(super) fn check_function_signature_call(
        &mut self,
        expr: &Expr,
        resolved: &ResolvedFunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        let signature = &resolved.signature;
        if let Some(builtin) = builtin_function(signature) {
            return self.check_builtin_function_call(
                span,
                span,
                expr,
                builtin,
                BuiltinCallTypeArgs::Bracket(&[]),
                args,
            );
        }
        if signature.generics.is_empty() {
            let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
            self.check_direct_call_args(span, args, &params, signature.is_variadic);
            self.record_resolved_node_call(
                span,
                &expr.node_key,
                ResolvedCall::Function(resolved.def_id),
            );
            return signature.return_type;
        }
        self.check_inferred_generic_function_call(expr, resolved.def_id, signature, args, expected)
    }

    pub(super) fn check_explicit_generic_call(
        &mut self,
        expr: &Expr,
        bracket_expr: &Expr,
        callee: &Expr,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_explicit_generic_field_method_call(
                expr, lhs, name, type_args, args, expected,
            )
        {
            self.record_bracket_suffix_node_resolution(
                bracket_expr,
                BracketSuffixResolution::GenericCall,
            );
            return return_type;
        }
        if let Some(builtin) = std_builtin_function(callee) {
            self.record_bracket_suffix_node_resolution(
                bracket_expr,
                BracketSuffixResolution::GenericCall,
            );
            return self.check_builtin_function_call(
                expr.span,
                bracket_expr.span,
                expr,
                builtin,
                BuiltinCallTypeArgs::Bracket(type_args),
                args,
            );
        }
        if let ExprKind::Qualified { lhs, name } = &callee.kind
            && let Some(return_type) = self
                .check_explicit_generic_associated_call(expr, lhs, name, type_args, args, expected)
        {
            self.record_bracket_suffix_node_resolution(
                bracket_expr,
                BracketSuffixResolution::GenericCall,
            );
            return return_type;
        }
        if let Some(resolved) = self.qualified_callee_signature(callee) {
            self.record_bracket_suffix_node_resolution(
                bracket_expr,
                BracketSuffixResolution::GenericCall,
            );
            if let Some(builtin) = builtin_function(&resolved.signature) {
                return self.check_builtin_function_call(
                    expr.span,
                    bracket_expr.span,
                    expr,
                    builtin,
                    BuiltinCallTypeArgs::Bracket(type_args),
                    args,
                );
            }
            return self.check_instantiated_function_call(
                expr,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
                expected,
            );
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            self.record_bracket_suffix_node_resolution(
                bracket_expr,
                BracketSuffixResolution::GenericCall,
            );
            if let Some(builtin) = builtin_function(&resolved.signature) {
                return self.check_builtin_function_call(
                    expr.span,
                    bracket_expr.span,
                    expr,
                    builtin,
                    BuiltinCallTypeArgs::Bracket(type_args),
                    args,
                );
            }
            return self.check_instantiated_function_call(
                expr,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
                expected,
            );
        }
        let callee_ty = self.check_bracket_suffix_expr(bracket_expr, callee, type_args, None);
        self.record_expr_node_type(bracket_expr, callee_ty);
        self.check_function_pointer_call_with_callee_ty(expr, callee_ty, args)
    }

    pub(super) fn check_function_pointer_call_with_callee_ty(
        &mut self,
        expr: &Expr,
        callee_ty: InternedTyId,
        args: &[Expr],
    ) -> InternedTyId {
        let span = expr.span;
        let callee_ty = self.normalize_aliases_in_type(callee_ty);
        match self.expect_ty_kind(callee_ty).clone() {
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                self.check_direct_call_args(span, args, &params, is_variadic);
                self.record_resolved_node_call(span, &expr.node_key, ResolvedCall::FunctionPointer);
                return_type
            }
            TyKind::ClosureState {
                params,
                return_type,
                ..
            } => {
                self.check_direct_call_args(span, args, &params, false);
                self.record_resolved_node_call(span, &expr.node_key, ResolvedCall::Closure);
                return_type
            }
            TyKind::Callable {
                params,
                return_type,
                ..
            } => {
                self.check_direct_call_args(span, args, &params, false);
                self.record_resolved_node_call(span, &expr.node_key, ResolvedCall::Callable);
                return_type
            }
            TyKind::Error => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.error()
            }
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "callee is not callable",
                ));
                for arg in args {
                    self.check_expr(arg);
                }
                self.error()
            }
        }
    }

    fn check_instantiated_function_call(
        &mut self,
        expr: &Expr,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        let Some(lowered_args) = self.lower_call_bracket_args_for_generic_params(
            span,
            &signature.generic_params,
            type_args,
        ) else {
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        };
        let mut substitutions = lowered_args.type_substitutions;
        let mut const_substitutions = lowered_args.const_substitutions;
        self.infer_generic_function_call_substitutions(
            span,
            signature,
            args,
            expected,
            &mut substitutions,
            &mut const_substitutions,
        );
        let type_generics = type_generic_names(signature);
        let Some(instance_args) =
            self.complete_generic_function_instance_args(span, &type_generics, &substitutions)
        else {
            return self.error();
        };
        let Some(const_instance_args) = self.complete_const_instance_args_for_generic_params(
            span,
            &signature.generic_params,
            &const_substitutions,
        ) else {
            return self.error();
        };
        self.record_generic_instantiation_with_const_args(
            def_id,
            &instance_args,
            &const_instance_args,
            span,
        );
        self.record_resolved_node_call(
            span,
            &expr.node_key,
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: self.defs.module_id,
                args: instance_args,
                const_args: const_instance_args,
            },
        );
        self.check_instantiated_generic_function_call_args(
            span,
            signature,
            args,
            &substitutions,
            &const_substitutions,
        );
        let return_type = self.substitute_generics_and_consts(
            signature.return_type,
            &substitutions,
            &const_substitutions,
        );
        let return_type = self.normalize_projection(return_type);
        self.normalize_aliases_in_type(return_type)
    }

    fn check_inferred_generic_function_call(
        &mut self,
        expr: &Expr,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        self.infer_generic_function_call_substitutions(
            span,
            signature,
            args,
            expected,
            &mut substitutions,
            &mut const_substitutions,
        );
        let type_generics = type_generic_names(signature);
        let Some(instance_args) =
            self.complete_generic_function_instance_args(span, &type_generics, &substitutions)
        else {
            return self.error();
        };
        let Some(const_instance_args) = self.complete_const_instance_args_for_generic_params(
            span,
            &signature.generic_params,
            &const_substitutions,
        ) else {
            return self.error();
        };
        self.record_generic_instantiation_with_const_args(
            def_id,
            &instance_args,
            &const_instance_args,
            span,
        );
        self.record_resolved_node_call(
            span,
            &expr.node_key,
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: self.defs.module_id,
                args: instance_args,
                const_args: const_instance_args,
            },
        );

        self.check_instantiated_generic_function_call_args(
            span,
            signature,
            args,
            &substitutions,
            &const_substitutions,
        );
        let return_type = self.substitute_generics_and_consts(
            signature.return_type,
            &substitutions,
            &const_substitutions,
        );
        let return_type = self.normalize_projection(return_type);
        self.normalize_aliases_in_type(return_type)
    }

    fn infer_generic_function_call_substitutions(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) {
        let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
        if let Some(expected) = expected.and_then(|expected| self.generic_call_expected(expected)) {
            self.infer_generics_from_type(signature.return_type, expected, substitutions, span);
        }
        self.infer_generic_function_call_substitutions_from_where_predicates(
            signature,
            args,
            substitutions,
        );
        self.check_call_arg_count(span, args.len(), params.len(), signature.is_variadic);
        for (index, arg) in args.iter().enumerate() {
            let Some(param) = params.get(index).copied() else {
                self.check_expr(arg);
                continue;
            };
            let inferred_from_closure =
                self.infer_generics_from_closure_signature(param, arg, substitutions, arg.span);
            let substituted_param =
                self.substitute_generics_and_consts(param, substitutions, const_substitutions);
            let expected = self.generic_call_expected(substituted_param);
            let actual = if let Some(expected) = expected {
                self.check_expr_with_expected(arg, Some(expected))
            } else if inferred_from_closure {
                // A later argument or the closure's known result may still
                // determine the generic callable signature. The instantiated
                // argument pass performs the one authoritative body check.
                continue;
            } else if matches!(arg.kind, ExprKind::ArrayLiteral { .. }) {
                self.infer_array_literal_expr(arg)
            } else {
                self.check_expr(arg)
            };
            let closure_shape_matches = self.inferred_closure_signature(arg).is_none()
                || self.generic_pattern_accepts_type_shape(param, actual);
            if closure_shape_matches {
                self.infer_generics_from_type(param, actual, substitutions, arg.span);
                if self.generic_pattern_accepts_type_shape(param, actual) {
                    // Const inference is staged behind a complete structural
                    // probe. A mismatch in a later tuple field or associated
                    // binding must not leak values collected from a prefix.
                    let mut staged = const_substitutions.clone();
                    self.infer_const_generics_from_type(param, actual, &mut staged, arg.span);
                    *const_substitutions = staged;
                }
            }
            self.infer_generic_function_call_substitutions_from_where_predicates(
                signature,
                args,
                substitutions,
            );
        }
    }

    pub(crate) fn infer_generics_from_closure_signature(
        &mut self,
        pattern: InternedTyId,
        expr: &Expr,
        substitutions: &mut SymbolMap<InternedTyId>,
        span: Span,
    ) -> bool {
        let Some(signature) = self.inferred_closure_signature(expr).cloned() else {
            return false;
        };
        // `&Fn` is canonicalized to `TyKind::Callable`, so the address
        // operator is the only remaining source of the closure state's
        // mutability during pre-check inference.
        let closure_address_readonly = match &expr.kind {
            ExprKind::Unary {
                op: nia_ast::UnaryOp::RefReadOnly,
                ..
            } => Some(true),
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                ..
            } => Some(false),
            _ => None,
        };
        let pattern = self.normalization.normalize(pattern);
        let Some((params, return_type)) = (match self.interner.get(pattern).cloned() {
            Some(TyKind::Callable {
                is_readonly: pattern_readonly,
                params,
                return_type,
            }) if closure_address_readonly.is_some_and(|actual_readonly| {
                pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly
            }) =>
            {
                Some((params, return_type))
            }
            Some(TyKind::CallablePointee {
                params,
                return_type,
            })
            | Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            }) => Some((params, return_type)),
            _ => None,
        }) else {
            return false;
        };
        if params.len() != signature.params.len() {
            return false;
        }
        // Closure inference is speculative: a structural mismatch in a later
        // parameter must not commit substitutions collected from an earlier
        // one. Probe the complete partial signature before mutating the map.
        if !params
            .iter()
            .zip(&signature.params)
            .all(|(pattern, actual)| self.generic_pattern_accepts_inferred_shape(*pattern, actual))
            || !self.generic_pattern_accepts_inferred_shape(return_type, &signature.return_type)
        {
            return false;
        }
        for (pattern, actual) in params.into_iter().zip(&signature.params) {
            self.infer_generics_from_inferred_type(pattern, actual, substitutions, span);
        }
        self.infer_generics_from_inferred_type(
            return_type,
            &signature.return_type,
            substitutions,
            span,
        );
        true
    }

    /// Checks whether closure-local partial type information can safely feed
    /// generic inference without publishing substitutions or diagnostics.
    fn generic_pattern_accepts_inferred_shape(
        &mut self,
        pattern: InternedTyId,
        actual: &crate::inference::InferredType,
    ) -> bool {
        use crate::inference::InferredType;

        if let Some(actual) = self.materialize_inferred_type(actual) {
            return self.generic_pattern_accepts_type_shape(pattern, actual);
        }
        let pattern = self.normalization.normalize(pattern);
        match (self.interner.get(pattern).cloned(), actual) {
            (_, InferredType::Unknown) => true,
            (Some(TyKind::Tuple(patterns)), InferredType::Tuple(actuals)) => {
                patterns.len() == actuals.len()
                    && patterns.iter().zip(actuals).all(|(pattern, actual)| {
                        self.generic_pattern_accepts_inferred_shape(*pattern, actual)
                    })
            }
            (
                Some(TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                }),
                InferredType::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) => {
                (pattern_readonly == *actual_readonly || pattern_readonly && !actual_readonly)
                    && self.generic_pattern_accepts_inferred_shape(pattern_elem, actual_elem)
            }
            (Some(TyKind::Optional { elem }), InferredType::Optional(actual)) => {
                self.generic_pattern_accepts_inferred_shape(elem, actual)
            }
            (
                Some(TyKind::ErrorUnion { error, value }),
                InferredType::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                },
            ) => {
                self.generic_pattern_accepts_inferred_shape(error, actual_error)
                    && self.generic_pattern_accepts_inferred_shape(value, actual_value)
            }
            (
                Some(TyKind::Callable {
                    params,
                    return_type,
                    ..
                })
                | Some(TyKind::CallablePointee {
                    params,
                    return_type,
                })
                | Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: false,
                }),
                InferredType::Callable {
                    params: actual_params,
                    return_type: actual_return,
                },
            ) => {
                params.len() == actual_params.len()
                    && params.iter().zip(actual_params).all(|(pattern, actual)| {
                        self.generic_pattern_accepts_inferred_shape(*pattern, actual)
                    })
                    && self.generic_pattern_accepts_inferred_shape(return_type, actual_return)
            }
            _ => false,
        }
    }

    /// Structural counterpart to [`Self::infer_generics_from_type`].
    ///
    /// It intentionally checks only shapes that can contain substitutions;
    /// concrete leaves use ordinary type matching. Unsupported generic shapes
    /// are rejected conservatively so speculative closure inference cannot
    /// leak a prefix of substitutions.
    pub(in crate::calls) fn generic_pattern_accepts_type_shape(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        let Some(pattern_kind) = self.interner.get(pattern).cloned() else {
            return false;
        };
        if matches!(pattern_kind, TyKind::GenericParam(_)) {
            return true;
        }
        if !self.type_contains_generic_param(pattern)
            && !self.type_contains_const_generic_param(pattern)
        {
            return self.types_match(pattern, actual);
        }
        match (pattern_kind, self.interner.get(actual).cloned()) {
            (
                TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            )
            | (
                TyKind::VolatilePointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            )
            | (
                TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                (pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly)
                    && self.generic_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
            (
                TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                // Slice values are lowered as pointers whose element is either
                // an array or the canonical slice-pointee marker. Inspect the
                // pointee to infer `T` without widening mutable references.
                if !(pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly) {
                    return false;
                }
                match self.interner.get(actual_elem).cloned() {
                    Some(TyKind::Array {
                        elem: actual_elem, ..
                    })
                    | Some(TyKind::SlicePointee { elem: actual_elem }) => {
                        self.generic_pattern_accepts_type_shape(pattern_elem, actual_elem)
                    }
                    _ => false,
                }
            }
            (
                TyKind::SlicePointee { elem: pattern_elem },
                Some(TyKind::SlicePointee { elem: actual_elem }),
            )
            | (
                TyKind::Optional { elem: pattern_elem },
                Some(TyKind::Optional { elem: actual_elem }),
            ) => self.generic_pattern_accepts_type_shape(pattern_elem, actual_elem),
            (
                TyKind::Array {
                    len: pattern_len,
                    elem: pattern_elem,
                },
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) => {
                self.const_generic_array_len_pattern_accepts(&pattern_len, &actual_len)
                    && self.generic_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
            (
                TyKind::Range {
                    kind: pattern_kind,
                    bound: pattern_bound,
                },
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }),
            ) => {
                pattern_kind == actual_kind
                    && match (pattern_bound, actual_bound) {
                        (Some(pattern), Some(actual)) => {
                            self.generic_pattern_accepts_type_shape(pattern, actual)
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            (TyKind::Tuple(patterns), Some(TyKind::Tuple(actuals))) => {
                patterns.len() == actuals.len()
                    && patterns.iter().zip(actuals).all(|(pattern, actual)| {
                        self.generic_pattern_accepts_type_shape(*pattern, actual)
                    })
            }
            (
                TyKind::ErrorUnion {
                    error: pattern_error,
                    value: pattern_value,
                },
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }),
            ) => {
                self.generic_pattern_accepts_type_shape(pattern_error, actual_error)
                    && self.generic_pattern_accepts_type_shape(pattern_value, actual_value)
            }
            (
                TyKind::FunctionPointer {
                    params: pattern_params,
                    return_type: pattern_return,
                    is_variadic: pattern_variadic,
                },
                Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }),
            ) => {
                pattern_variadic == actual_variadic
                    && pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.generic_pattern_accepts_type_shape(pattern_return, actual_return)
            }
            (
                TyKind::Callable {
                    is_readonly: pattern_readonly,
                    params: pattern_params,
                    return_type: pattern_return,
                },
                Some(TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) => {
                (pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly)
                    && pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.generic_pattern_accepts_type_shape(pattern_return, actual_return)
            }
            (
                TyKind::Callable {
                    is_readonly: pattern_readonly,
                    params: pattern_params,
                    return_type: pattern_return,
                },
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem,
                }),
            ) => {
                let Some(TyKind::ClosureState {
                    params: actual_params,
                    return_type: actual_return,
                    ..
                }) = self.interner.get(elem).cloned()
                else {
                    return false;
                };
                (pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly)
                    && pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.generic_pattern_accepts_type_shape(pattern_return, actual_return)
            }
            (
                TyKind::CallablePointee {
                    params: pattern_params,
                    return_type: pattern_return,
                },
                Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) => {
                pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.generic_pattern_accepts_type_shape(pattern_return, actual_return)
            }
            (
                TyKind::Nominal {
                    def_id: pattern_def,
                    args: pattern_args,
                    const_args: pattern_const_args,
                },
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }),
            ) => {
                pattern_def == actual_def
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
            }
            (
                TyKind::BuiltinTrait {
                    trait_id: pattern_trait,
                    args: pattern_args,
                },
                Some(TyKind::BuiltinTrait {
                    trait_id: actual_trait,
                    args: actual_args,
                }),
            ) => {
                pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
            }
            (
                TyKind::TraitObject {
                    is_readonly: pattern_readonly,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                },
                Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) => {
                pattern_readonly == actual_readonly
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && pattern_bindings.len() == actual_bindings.len()
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self
                        .generic_binding_patterns_accept_shapes(&pattern_bindings, &actual_bindings)
            }
            (
                TyKind::TraitObjectPointee {
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                },
                Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) => {
                pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && pattern_bindings.len() == actual_bindings.len()
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self
                        .generic_binding_patterns_accept_shapes(&pattern_bindings, &actual_bindings)
            }
            (
                TyKind::Projection {
                    self_ty: pattern_self,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    name: pattern_name,
                },
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }),
            ) => {
                pattern_trait == actual_trait
                    && pattern_name == actual_name
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && self.generic_pattern_accepts_type_shape(pattern_self, actual_self)
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.generic_pattern_accepts_type_shape(*pattern, actual)
                        })
            }
            _ => false,
        }
    }

    fn generic_binding_patterns_accept_shapes(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
    ) -> bool {
        if patterns.len() != actuals.len() {
            return false;
        }
        self.generic_binding_patterns_accept_shapes_inner(
            patterns,
            actuals,
            0,
            &mut vec![false; actuals.len()],
        )
    }

    fn generic_binding_patterns_accept_shapes_inner(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
    ) -> bool {
        let Some(pattern) = patterns.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual) in actuals.iter().enumerate() {
            if used[actual_index]
                || pattern.name != actual.name
                || pattern.trait_id != actual.trait_id
                || pattern.trait_args.len() != actual.trait_args.len()
                || !pattern.trait_args.iter().zip(actual.trait_args.iter()).all(
                    |(pattern, actual)| self.generic_pattern_accepts_type_shape(*pattern, *actual),
                )
                || !self.const_generic_arg_patterns_accept(
                    &pattern.trait_const_args,
                    &actual.trait_const_args,
                )
                || !self.generic_pattern_accepts_type_shape(pattern.ty, actual.ty)
            {
                continue;
            }
            used[actual_index] = true;
            let matched = self.generic_binding_patterns_accept_shapes_inner(
                patterns,
                actuals,
                pattern_index + 1,
                used,
            );
            used[actual_index] = false;
            if matched {
                return true;
            }
        }
        false
    }

    fn const_generic_array_len_pattern_accepts(
        &mut self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
    ) -> bool {
        if matches!(pattern, ArrayLenTy::GenericParam(_)) {
            return self
                .const_generic_value_from_array_len(actual.clone())
                .is_some();
        }
        if pattern == actual {
            return true;
        }
        if let (
            ArrayLenTy::Builtin {
                builtin: pattern_builtin,
                ty: pattern_ty,
            },
            ArrayLenTy::Builtin {
                builtin: actual_builtin,
                ty: actual_ty,
            },
        ) = (pattern, actual)
            && pattern_builtin == actual_builtin
            && (self.type_contains_generic_param(*pattern_ty)
                || self.type_contains_const_generic_param(*pattern_ty)
                || self.types_match(*pattern_ty, *actual_ty))
        {
            return true;
        }
        let pattern = self.array_len_value(Span::default(), pattern).ok();
        let actual = self.array_len_value(Span::default(), actual).ok();
        pattern.is_some() && pattern == actual
    }

    fn const_generic_arg_patterns_accept(
        &mut self,
        patterns: &[ConstGenericArg],
        actuals: &[ConstGenericArg],
    ) -> bool {
        patterns.len() == actuals.len()
            && patterns.iter().zip(actuals).all(|(pattern, actual)| {
                self.generic_pattern_accepts_type_shape(pattern.ty, actual.ty)
                    && (matches!(pattern.value, ConstGenericValue::GenericParam(_))
                        || self.const_generic_args_match(pattern, actual))
            })
    }

    /// Probes a complete associated binding while inferring type parameters.
    ///
    /// Binding keys are not unique when trait arguments differ, so matching
    /// only the key can select an incompatible value and prevent a later
    /// candidate from being considered. Reuse the method matcher, which
    /// stages all type and const substitutions until key and value agree.
    fn try_infer_associated_type_binding(
        &mut self,
        pattern: &AssociatedTypeBindingTy,
        actual: &AssociatedTypeBindingTy,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> bool {
        let mut candidate = substitutions.clone();
        let mut const_substitutions = SymbolMap::default();
        if !self.try_match_associated_type_binding(
            pattern,
            actual,
            &mut candidate,
            &mut const_substitutions,
        ) {
            return false;
        }
        *substitutions = candidate;
        true
    }

    /// Probes a complete associated binding while inferring const parameters.
    /// Type substitutions are local to the probe because this pass publishes
    /// only the const map.
    fn try_infer_associated_const_binding(
        &mut self,
        pattern: &AssociatedTypeBindingTy,
        actual: &AssociatedTypeBindingTy,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        let mut type_substitutions = SymbolMap::default();
        let mut candidate = substitutions.clone();
        if !self.try_match_associated_type_binding(
            pattern,
            actual,
            &mut type_substitutions,
            &mut candidate,
        ) {
            return false;
        }
        *substitutions = candidate;
        true
    }

    /// Matches all associated bindings as one transaction.
    ///
    /// A binding key is not necessarily unique: trait arguments and the
    /// associated value can distinguish otherwise identical names. Matching
    /// each pattern with `find` is therefore order-dependent and may consume
    /// the only candidate needed by a later pattern. The recursive search
    /// keeps actual bindings distinct and publishes substitutions only after
    /// a complete assignment succeeds.
    fn infer_associated_type_bindings(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> Option<Vec<usize>> {
        if patterns.len() != actuals.len() {
            return None;
        }
        let mut used = vec![false; actuals.len()];
        let mut matched = vec![0; patterns.len()];
        let mut candidate = substitutions.clone();
        if self.infer_associated_type_bindings_inner(
            patterns,
            actuals,
            0,
            &mut used,
            &mut matched,
            &mut candidate,
        ) {
            *substitutions = candidate;
            Some(matched)
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_associated_type_bindings_inner(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
        matched: &mut [usize],
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> bool {
        let Some(pattern) = patterns.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual) in actuals.iter().enumerate() {
            if used[actual_index] {
                continue;
            }
            let mut candidate = substitutions.clone();
            if !self.try_infer_associated_type_binding(pattern, actual, &mut candidate) {
                continue;
            }
            used[actual_index] = true;
            matched[pattern_index] = actual_index;
            let success = self.infer_associated_type_bindings_inner(
                patterns,
                actuals,
                pattern_index + 1,
                used,
                matched,
                &mut candidate,
            );
            used[actual_index] = false;
            if success {
                *substitutions = candidate;
                return true;
            }
        }
        false
    }

    /// Const-generic counterpart of [`Self::infer_associated_type_bindings`].
    fn infer_associated_const_bindings(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Option<Vec<usize>> {
        if patterns.len() != actuals.len() {
            return None;
        }
        let mut used = vec![false; actuals.len()];
        let mut matched = vec![0; patterns.len()];
        let mut candidate = substitutions.clone();
        if self.infer_associated_const_bindings_inner(
            patterns,
            actuals,
            0,
            &mut used,
            &mut matched,
            &mut candidate,
        ) {
            *substitutions = candidate;
            Some(matched)
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_associated_const_bindings_inner(
        &mut self,
        patterns: &[AssociatedTypeBindingTy],
        actuals: &[AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
        matched: &mut [usize],
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        let Some(pattern) = patterns.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual) in actuals.iter().enumerate() {
            if used[actual_index] {
                continue;
            }
            let mut candidate = substitutions.clone();
            if !self.try_infer_associated_const_binding(pattern, actual, &mut candidate) {
                continue;
            }
            used[actual_index] = true;
            matched[pattern_index] = actual_index;
            let success = self.infer_associated_const_bindings_inner(
                patterns,
                actuals,
                pattern_index + 1,
                used,
                matched,
                &mut candidate,
            );
            used[actual_index] = false;
            if success {
                *substitutions = candidate;
                return true;
            }
        }
        false
    }

    pub(crate) fn materialize_inferred_type(
        &mut self,
        inferred: &crate::inference::InferredType,
    ) -> Option<InternedTyId> {
        use crate::inference::InferredType;

        match inferred {
            InferredType::Unknown => None,
            InferredType::Known(ty) => Some(*ty),
            InferredType::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| self.materialize_inferred_type(elem))
                    .collect::<Option<Vec<_>>>()?;
                Some(self.interner.intern(TyKind::Tuple(elems)))
            }
            InferredType::Pointer { is_readonly, elem } => {
                let elem = self.materialize_inferred_type(elem)?;
                Some(self.interner.intern(TyKind::Pointer {
                    is_readonly: *is_readonly,
                    elem,
                }))
            }
            InferredType::Optional(elem) => {
                let elem = self.materialize_inferred_type(elem)?;
                Some(self.interner.intern(TyKind::Optional { elem }))
            }
            InferredType::ErrorUnion { error, value } => {
                let error = self.materialize_inferred_type(error)?;
                let value = self.materialize_inferred_type(value)?;
                Some(self.interner.intern(TyKind::ErrorUnion { error, value }))
            }
            InferredType::Callable {
                params: _,
                return_type: _,
            } => None,
        }
    }

    fn infer_generics_from_inferred_type(
        &mut self,
        pattern: InternedTyId,
        actual: &crate::inference::InferredType,
        substitutions: &mut SymbolMap<InternedTyId>,
        span: Span,
    ) {
        use crate::inference::InferredType;

        if let Some(actual) = self.materialize_inferred_type(actual) {
            self.infer_generics_from_type(pattern, actual, substitutions, span);
            return;
        }
        let pattern = self.normalization.normalize(pattern);
        match (self.interner.get(pattern).cloned(), actual) {
            (Some(TyKind::Tuple(patterns)), InferredType::Tuple(actuals))
                if patterns.len() == actuals.len() =>
            {
                for (pattern, actual) in patterns.into_iter().zip(actuals) {
                    self.infer_generics_from_inferred_type(pattern, actual, substitutions, span);
                }
            }
            (
                Some(TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                }),
                InferredType::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) if pattern_readonly == *actual_readonly || pattern_readonly && !actual_readonly => {
                self.infer_generics_from_inferred_type(
                    pattern_elem,
                    actual_elem,
                    substitutions,
                    span,
                );
            }
            (Some(TyKind::Optional { elem }), InferredType::Optional(actual)) => {
                self.infer_generics_from_inferred_type(elem, actual, substitutions, span);
            }
            (
                Some(TyKind::ErrorUnion { error, value }),
                InferredType::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                },
            ) => {
                self.infer_generics_from_inferred_type(error, actual_error, substitutions, span);
                self.infer_generics_from_inferred_type(value, actual_value, substitutions, span);
            }
            (
                Some(TyKind::Callable {
                    params,
                    return_type,
                    ..
                })
                | Some(TyKind::CallablePointee {
                    params,
                    return_type,
                })
                | Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: false,
                }),
                InferredType::Callable {
                    params: actual_params,
                    return_type: actual_return,
                },
            ) if params.len() == actual_params.len() => {
                for (pattern, actual) in params.into_iter().zip(actual_params) {
                    self.infer_generics_from_inferred_type(pattern, actual, substitutions, span);
                }
                self.infer_generics_from_inferred_type(
                    return_type,
                    actual_return,
                    substitutions,
                    span,
                );
            }
            _ => {}
        }
    }

    fn infer_generic_function_call_substitutions_from_where_predicates(
        &mut self,
        signature: &FunctionSignature,
        args: &[Expr],
        substitutions: &mut SymbolMap<InternedTyId>,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for predicate in &signature.where_predicates {
                let candidates = self
                    .infer_where_predicate_candidates(predicate, substitutions)
                    .into_iter()
                    .filter(|candidate| {
                        self.where_candidate_matches_call_args(signature, args, candidate)
                    })
                    .collect::<Vec<_>>();
                let Some(candidate) = self.single_where_candidate(&candidates) else {
                    continue;
                };
                for (generic, ty) in candidate {
                    if !substitutions.contains_key(generic) {
                        substitutions.insert(*generic, *ty);
                        changed = true;
                    }
                }
            }
        }
    }

    fn where_candidate_matches_call_args(
        &mut self,
        signature: &FunctionSignature,
        args: &[Expr],
        candidate: &SymbolMap<InternedTyId>,
    ) -> bool {
        for (index, arg) in args.iter().enumerate() {
            let Some(param) = signature.params.get(index).map(|param| param.ty) else {
                continue;
            };
            let expected = self.substitute_generics(param, candidate);
            if self.type_contains_generic_param(expected) {
                continue;
            }
            if !self.expr_can_match_expected(arg, expected) {
                return false;
            }
        }
        true
    }

    pub(crate) fn expr_can_match_expected(&mut self, expr: &Expr, expected: InternedTyId) -> bool {
        let expected = self.normalization.normalize(expected);
        match (&expr.kind, self.interner.get(expected).cloned()) {
            (
                ExprKind::Unary {
                    op: UnaryOp::RefReadOnly,
                    expr: inner,
                },
                _,
            ) if Self::reference_target_has_standalone_type(inner) => {
                let actual = self.check_expr(expr);
                self.type_can_match_call_expected(expected, actual)
            }
            (
                ExprKind::Unary {
                    op: UnaryOp::RefReadOnly,
                    expr: inner,
                },
                _,
            ) if matches!(&inner.kind, ExprKind::String(_) | ExprKind::ByteString(_)) => {
                let array = match &inner.kind {
                    ExprKind::String(literal) => self.string_literal_array_type(literal),
                    ExprKind::ByteString(literal) => self.byte_string_literal_array_type(literal),
                    _ => unreachable!(),
                };
                let actual = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: array,
                });
                self.type_can_match_call_expected(expected, actual)
            }
            (ExprKind::String(_), Some(TyKind::Array { elem, .. })) => {
                self.types_match(elem, self.primitive(PrimitiveTy::Char))
            }
            (ExprKind::ByteString(_), Some(TyKind::Array { elem, .. })) => {
                self.types_match(elem, self.primitive(PrimitiveTy::U8))
            }
            (ExprKind::String(_), _) | (ExprKind::ByteString(_), _) => false,
            _ => true,
        }
    }

    fn type_can_match_call_expected(
        &mut self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        if self.types_match(expected, actual) {
            return true;
        }
        let Some((_, actual_slice, actual_readonly)) = self.pointer_array_slice_type(actual) else {
            return false;
        };
        if self.types_match(expected, actual_slice) {
            return true;
        }
        if actual_readonly {
            return false;
        }
        let Some(TyKind::Slice { elem, .. }) = self.interner.get(actual_slice).cloned() else {
            return false;
        };
        let readonly_slice = self.interner.intern(TyKind::Slice {
            is_readonly: true,
            elem,
        });
        self.types_match(expected, readonly_slice)
    }

    fn reference_target_has_standalone_type(expr: &Expr) -> bool {
        matches!(
            &expr.kind,
            ExprKind::Ident(_)
                | ExprKind::Qualified { .. }
                | ExprKind::Field { .. }
                | ExprKind::Index { .. }
        )
    }

    fn complete_generic_function_instance_args(
        &mut self,
        span: Span,
        generics: &[SymbolId],
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<Vec<InternedTyId>> {
        self.complete_instance_args_for_generics(span, generics, substitutions)
    }

    fn check_instantiated_generic_function_call_args(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        args: &[Expr],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) {
        let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
        let instantiated_params: Vec<InternedTyId> = params
            .iter()
            .map(|param| {
                self.substitute_generics_and_consts(*param, substitutions, const_substitutions)
            })
            .collect();
        self.check_where_predicates_hold(
            &signature.where_predicates,
            substitutions,
            const_substitutions,
            span,
        );
        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = instantiated_params.get(index).copied() {
                let actual = self.check_expr_with_expected(arg, Some(expected));
                self.expect_expr_type(arg, expected, actual, "call argument");
            }
        }
    }

    fn generic_call_expected(&self, ty: InternedTyId) -> Option<InternedTyId> {
        if self.type_contains_generic_param(ty) || self.type_contains_const_generic_param(ty) {
            None
        } else {
            Some(ty)
        }
    }

    /// Returns whether a canonical type still contains a const parameter that
    /// must be substituted before the type can guide expression checking.
    ///
    /// This mirrors [`Self::type_contains_generic_param`] across every type
    /// container. Treating `[T; N]` as a complete expected type before `N` is
    /// inferred causes eager tuple/aggregate diagnostics even when the later
    /// instantiated argument type is valid.
    pub(crate) fn type_contains_const_generic_param(&self, ty: InternedTyId) -> bool {
        let const_arg_contains_param = |arg: &ConstGenericArg| {
            matches!(arg.value, ConstGenericValue::GenericParam(_))
                || self.type_contains_const_generic_param(arg.ty)
        };
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.type_contains_const_generic_param(*elem),
            Some(TyKind::Array { len, elem }) => {
                matches!(len, ArrayLenTy::GenericParam(_))
                    || matches!(len, ArrayLenTy::Builtin { ty, .. } if self.type_contains_const_generic_param(*ty))
                    || self.type_contains_const_generic_param(*elem)
            }
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_const_generic_param(bound))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                params
                    .iter()
                    .any(|param| self.type_contains_const_generic_param(*param))
                    || self.type_contains_const_generic_param(*return_type)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_const_generic_param(*error)
                    || self.type_contains_const_generic_param(*value)
            }
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.type_contains_const_generic_param(*elem)),
            Some(TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            }) => {
                captures
                    .iter()
                    .chain(params)
                    .any(|ty| self.type_contains_const_generic_param(*ty))
                    || self.type_contains_const_generic_param(*return_type)
            }
            Some(TyKind::Nominal {
                args, const_args, ..
            }) => {
                args.iter()
                    .any(|arg| self.type_contains_const_generic_param(*arg))
                    || const_args.iter().any(const_arg_contains_param)
            }
            Some(TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.type_contains_const_generic_param(*arg)),
            Some(TyKind::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.type_contains_const_generic_param(*arg))
                    || trait_const_args.iter().any(const_arg_contains_param)
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.type_contains_const_generic_param(*arg))
                            || binding
                                .trait_const_args
                                .iter()
                                .any(const_arg_contains_param)
                            || self.type_contains_const_generic_param(binding.ty)
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.type_contains_const_generic_param(*self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.type_contains_const_generic_param(*arg))
                    || trait_const_args.iter().any(const_arg_contains_param)
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_)
                | TyKind::SelfParam,
            )
            | None => false,
        }
    }

    pub(crate) fn type_contains_generic_param(&self, ty: InternedTyId) -> bool {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::GenericParam(_) | TyKind::SelfParam) => true,
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.type_contains_generic_param(*elem),
            Some(TyKind::Array { len, elem }) => {
                self.type_contains_generic_param(*elem)
                    || matches!(len, ArrayLenTy::Builtin { ty, .. }
                        if self.type_contains_generic_param(*ty))
            }
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_generic_param(bound))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                params
                    .iter()
                    .any(|param| self.type_contains_generic_param(*param))
                    || self.type_contains_generic_param(*return_type)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_generic_param(*error) || self.type_contains_generic_param(*value)
            }
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.type_contains_generic_param(*elem)),
            Some(TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            }) => {
                captures
                    .iter()
                    .chain(params)
                    .any(|ty| self.type_contains_generic_param(*ty))
                    || self.type_contains_generic_param(*return_type)
            }
            Some(TyKind::Nominal {
                args, const_args, ..
            }) => {
                args.iter()
                    .any(|arg| self.type_contains_generic_param(*arg))
                    || const_args
                        .iter()
                        .any(|arg| self.type_contains_generic_param(arg.ty))
            }
            Some(TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.type_contains_generic_param(*arg)),
            Some(TyKind::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.type_contains_generic_param(*arg))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.type_contains_generic_param(arg.ty))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.type_contains_generic_param(*arg))
                            || binding
                                .trait_const_args
                                .iter()
                                .any(|arg| self.type_contains_generic_param(arg.ty))
                            || self.type_contains_generic_param(binding.ty)
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.type_contains_generic_param(*self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.type_contains_generic_param(*arg))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.type_contains_generic_param(arg.ty))
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => false,
        }
    }

    pub(crate) fn infer_generics_from_type(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
        span: Span,
    ) {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        match self.interner.get(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    if self.generic_substitution_is_self_mapping(&name, existing) {
                        substitutions.insert(name, actual);
                    } else if !self.types_match(existing, actual) {
                        let name = self.symbol_name(name);
                        self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                            span,
                            format!(
                                "conflicting inferred type for generic parameter `{name}`: expected {}, got {}",
                                self.ty_name(existing),
                                self.ty_name(actual)
                            ),
                        ));
                    }
                } else {
                    substitutions.insert(name, actual);
                }
            }
            Some(TyKind::SelfParam) => {}
            Some(TyKind::BuiltinType(_)) => {}
            Some(TyKind::Opaque) => {}
            Some(TyKind::Pointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => {
                if let Some(TyKind::Pointer {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && (pattern_const == actual_const || pattern_const && !actual_const)
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::VolatilePointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => {
                if let Some(TyKind::VolatilePointer {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && (pattern_const == actual_const || pattern_const && !actual_const)
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::Slice {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => {
                // Generic inference sees the pre-coercion pointer-to-array
                // shape; peel that representation before inferring the slice
                // element while preserving readonly compatibility.
                if let Some(TyKind::Slice {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && (pattern_const == actual_const || pattern_const && !actual_const)
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                } else if let Some(TyKind::Pointer {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && (pattern_const == actual_const || pattern_const && !actual_const)
                {
                    let actual_elem = self.interner.get(actual_elem).cloned();
                    if let Some(
                        TyKind::Array {
                            elem: actual_elem, ..
                        }
                        | TyKind::SlicePointee { elem: actual_elem },
                    ) = actual_elem
                    {
                        self.infer_generics_from_type(
                            pattern_elem,
                            actual_elem,
                            substitutions,
                            span,
                        );
                    }
                }
            }
            Some(TyKind::SlicePointee { elem: pattern_elem }) => {
                if let Some(TyKind::SlicePointee { elem: actual_elem }) =
                    self.interner.get(actual).cloned()
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => {
                if let Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                    ..
                }) = self.interner.get(actual).cloned()
                {
                    if let (
                        ArrayLenTy::Builtin {
                            builtin: pattern_builtin,
                            ty: pattern_ty,
                        },
                        ArrayLenTy::Builtin {
                            builtin: actual_builtin,
                            ty: actual_ty,
                        },
                    ) = (&pattern_len, &actual_len)
                        && pattern_builtin == actual_builtin
                    {
                        self.infer_generics_from_type(*pattern_ty, *actual_ty, substitutions, span);
                    }
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => {
                if let Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) = self.interner.get(actual).cloned()
                    && pattern_kind == actual_kind
                    && let (Some(pattern_bound), Some(actual_bound)) = (pattern_bound, actual_bound)
                {
                    self.infer_generics_from_type(pattern_bound, actual_bound, substitutions, span);
                }
            }
            Some(TyKind::FunctionPointer {
                params: pattern_params,
                return_type: pattern_return,
                is_variadic: pattern_variadic,
            }) => {
                if let Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }) = self.interner.get(actual).cloned()
                    && pattern_params.len() == actual_params.len()
                    && pattern_variadic == actual_variadic
                {
                    for (pattern, actual) in pattern_params.iter().zip(actual_params.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    self.infer_generics_from_type(
                        pattern_return,
                        actual_return,
                        substitutions,
                        span,
                    );
                }
            }
            Some(TyKind::Callable {
                is_readonly: pattern_readonly,
                params: pattern_params,
                return_type: pattern_return,
            }) => {
                let actual_callable = match self.interner.get(actual).cloned() {
                    Some(TyKind::Callable {
                        is_readonly,
                        params,
                        return_type,
                    }) => Some((is_readonly, params, return_type)),
                    Some(TyKind::Pointer { is_readonly, elem }) => {
                        match self.interner.get(elem).cloned() {
                            Some(TyKind::ClosureState {
                                params,
                                return_type,
                                ..
                            }) => Some((is_readonly, params, return_type)),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some((actual_readonly, actual_params, actual_return)) = actual_callable
                    // Match pointer coercion: a mutable state/view may be
                    // observed through a readonly callable, never vice versa.
                    && (pattern_readonly == actual_readonly
                        || pattern_readonly && !actual_readonly)
                    && pattern_params.len() == actual_params.len()
                {
                    for (pattern, actual) in pattern_params.iter().zip(actual_params.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    self.infer_generics_from_type(
                        pattern_return,
                        actual_return,
                        substitutions,
                        span,
                    );
                }
            }
            Some(TyKind::CallablePointee {
                params: pattern_params,
                return_type: pattern_return,
            }) => {
                if let Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                }) = self.interner.get(actual).cloned()
                    && pattern_params.len() == actual_params.len()
                {
                    for (pattern, actual) in pattern_params.iter().zip(actual_params.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    self.infer_generics_from_type(
                        pattern_return,
                        actual_return,
                        substitutions,
                        span,
                    );
                }
            }
            Some(TyKind::Optional { elem: pattern_elem }) => {
                if let Some(TyKind::Optional { elem: actual_elem }) =
                    self.interner.get(actual).cloned()
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::ErrorUnion {
                error: pattern_error,
                value: pattern_value,
            }) => {
                if let Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) = self.interner.get(actual).cloned()
                {
                    self.infer_generics_from_type(pattern_error, actual_error, substitutions, span);
                    self.infer_generics_from_type(pattern_value, actual_value, substitutions, span);
                }
            }
            Some(TyKind::Tuple(pattern_elems)) => {
                if let Some(TyKind::Tuple(actual_elems)) = self.interner.get(actual).cloned()
                    && pattern_elems.len() == actual_elems.len()
                {
                    for (pattern, actual) in pattern_elems.into_iter().zip(actual_elems) {
                        self.infer_generics_from_type(pattern, actual, substitutions, span);
                    }
                }
            }
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
                const_args: pattern_const_args,
            }) => {
                if let Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }) = self.interner.get(actual).cloned()
                    && pattern_def == actual_def
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args.iter())
                    {
                        self.infer_generics_from_type(pattern.ty, actual.ty, substitutions, span);
                    }
                }
            }
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => {
                if let Some(TyKind::BuiltinTrait {
                    trait_id: actual_trait,
                    args: actual_args,
                }) = self.interner.get(actual).cloned()
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                }
            }
            Some(TyKind::TraitObject {
                is_readonly: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => {
                if let Some(TyKind::TraitObject {
                    is_readonly: actual_const,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) = self.interner.get(actual).cloned()
                    && pattern_const == actual_const
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && pattern_bindings.len() == actual_bindings.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args.iter())
                    {
                        self.infer_generics_from_type(pattern.ty, actual.ty, substitutions, span);
                    }
                    if let Some(matches) = self.infer_associated_type_bindings(
                        &pattern_bindings,
                        &actual_bindings,
                        substitutions,
                    ) {
                        for (pattern_binding, actual_index) in pattern_bindings.iter().zip(matches)
                        {
                            let actual_binding = &actual_bindings[actual_index];
                            for (pattern, actual) in pattern_binding
                                .trait_args
                                .iter()
                                .zip(actual_binding.trait_args.iter())
                            {
                                self.infer_generics_from_type(
                                    *pattern,
                                    *actual,
                                    substitutions,
                                    span,
                                );
                            }
                            for (pattern, actual) in pattern_binding
                                .trait_const_args
                                .iter()
                                .zip(actual_binding.trait_const_args.iter())
                            {
                                self.infer_generics_from_type(
                                    pattern.ty,
                                    actual.ty,
                                    substitutions,
                                    span,
                                );
                            }
                            self.infer_generics_from_type(
                                pattern_binding.ty,
                                actual_binding.ty,
                                substitutions,
                                span,
                            );
                        }
                    }
                }
            }
            Some(TyKind::TraitObjectPointee {
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => {
                if let Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) = self.interner.get(actual).cloned()
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                    && pattern_bindings.len() == actual_bindings.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args.iter())
                    {
                        self.infer_generics_from_type(pattern.ty, actual.ty, substitutions, span);
                    }
                    if let Some(matches) = self.infer_associated_type_bindings(
                        &pattern_bindings,
                        &actual_bindings,
                        substitutions,
                    ) {
                        for (pattern_binding, actual_index) in pattern_bindings.iter().zip(matches)
                        {
                            let actual_binding = &actual_bindings[actual_index];
                            for (pattern, actual) in pattern_binding
                                .trait_args
                                .iter()
                                .zip(actual_binding.trait_args.iter())
                            {
                                self.infer_generics_from_type(
                                    *pattern,
                                    *actual,
                                    substitutions,
                                    span,
                                );
                            }
                            for (pattern, actual) in pattern_binding
                                .trait_const_args
                                .iter()
                                .zip(actual_binding.trait_const_args.iter())
                            {
                                self.infer_generics_from_type(
                                    pattern.ty,
                                    actual.ty,
                                    substitutions,
                                    span,
                                );
                            }
                            self.infer_generics_from_type(
                                pattern_binding.ty,
                                actual_binding.ty,
                                substitutions,
                                span,
                            );
                        }
                    }
                }
            }
            Some(TyKind::Projection {
                self_ty: pattern_self,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                name: pattern_name,
            }) => {
                if let Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }) = self.interner.get(actual).cloned()
                    && pattern_trait == actual_trait
                    && pattern_name == actual_name
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_arg_patterns_accept(&pattern_const_args, &actual_const_args)
                {
                    self.infer_generics_from_type(pattern_self, actual_self, substitutions, span);
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args.iter())
                    {
                        self.infer_generics_from_type(pattern.ty, actual.ty, substitutions, span);
                    }
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn generic_substitution_is_self_mapping(&self, name: &SymbolId, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::GenericParam(existing)) if existing == name
        )
    }

    pub(crate) fn infer_const_generics_from_type(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<ConstGenericArg>,
        span: Span,
    ) {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        match (
            self.interner.get(pattern).cloned(),
            self.interner.get(actual).cloned(),
        ) {
            (
                Some(TyKind::Array {
                    len: pattern_len,
                    elem: pattern_elem,
                }),
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) => {
                self.infer_const_generic_from_array_len(
                    pattern_len,
                    actual_len,
                    substitutions,
                    span,
                );
                self.infer_const_generics_from_type(pattern_elem, actual_elem, substitutions, span);
            }
            (
                Some(TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: left,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: right,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: pattern_readonly,
                    elem: left,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: right,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: left,
                }),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: right,
                }),
            ) if pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly => {
                self.infer_const_generics_from_type(left, right, substitutions, span)
            }
            (
                Some(TyKind::SlicePointee { elem: left }),
                Some(TyKind::SlicePointee { elem: right }),
            )
            | (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.infer_const_generics_from_type(left, right, substitutions, span)
            }
            (
                Some(TyKind::Range {
                    kind: pattern_kind,
                    bound: Some(pattern_bound),
                }),
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: Some(actual_bound),
                }),
            ) if pattern_kind == actual_kind => self.infer_const_generics_from_type(
                pattern_bound,
                actual_bound,
                substitutions,
                span,
            ),
            (
                Some(TyKind::FunctionPointer {
                    params: pattern_params,
                    return_type: pattern_return,
                    is_variadic: pattern_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }),
            ) if pattern_variadic == actual_variadic
                && pattern_params.len() == actual_params.len() =>
            {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                self.infer_const_generics_from_type(
                    pattern_return,
                    actual_return,
                    substitutions,
                    span,
                );
            }
            (
                Some(TyKind::Callable {
                    is_readonly: pattern_readonly,
                    params: pattern_params,
                    return_type: pattern_return,
                }),
                Some(TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) if (pattern_readonly == actual_readonly || pattern_readonly && !actual_readonly)
                && pattern_params.len() == actual_params.len() =>
            {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                self.infer_const_generics_from_type(
                    pattern_return,
                    actual_return,
                    substitutions,
                    span,
                );
            }
            (
                Some(TyKind::CallablePointee {
                    params: pattern_params,
                    return_type: pattern_return,
                }),
                Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                }),
            ) if pattern_params.len() == actual_params.len() => {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                self.infer_const_generics_from_type(
                    pattern_return,
                    actual_return,
                    substitutions,
                    span,
                );
            }
            (
                Some(TyKind::ErrorUnion {
                    error: pattern_error,
                    value: pattern_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }),
            ) => {
                self.infer_const_generics_from_type(
                    pattern_error,
                    actual_error,
                    substitutions,
                    span,
                );
                self.infer_const_generics_from_type(
                    pattern_value,
                    actual_value,
                    substitutions,
                    span,
                );
            }
            (Some(TyKind::Tuple(patterns)), Some(TyKind::Tuple(actuals)))
                if patterns.len() == actuals.len() =>
            {
                for (pattern, actual) in patterns.into_iter().zip(actuals) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
            }
            (
                Some(TyKind::Nominal {
                    def_id: pattern_def,
                    args: pattern_args,
                    const_args: pattern_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }),
            ) if pattern_def == actual_def
                && pattern_args.len() == actual_args.len()
                && pattern_const_args.len() == actual_const_args.len() =>
            {
                for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                    self.infer_const_generics_from_type(*pattern, *actual, substitutions, span);
                }
                for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args) {
                    self.infer_const_generic_from_arg(pattern, actual, substitutions, span);
                }
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: pattern_trait,
                    args: pattern_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: actual_trait,
                    args: actual_args,
                }),
            ) if pattern_trait == actual_trait && pattern_args.len() == actual_args.len() => {
                for (pattern, actual) in pattern_args.into_iter().zip(actual_args) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: pattern_readonly,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) if pattern_readonly == actual_readonly
                && pattern_trait == actual_trait
                && pattern_args.len() == actual_args.len()
                && pattern_const_args.len() == actual_const_args.len()
                && pattern_bindings.len() == actual_bindings.len() =>
            {
                for (pattern, actual) in pattern_args.into_iter().zip(actual_args) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args) {
                    self.infer_const_generic_from_arg(pattern, actual, substitutions, span);
                }
                if let Some(matches) = self.infer_associated_const_bindings(
                    &pattern_bindings,
                    &actual_bindings,
                    substitutions,
                ) {
                    for (pattern, actual_index) in pattern_bindings.iter().zip(matches) {
                        let actual = &actual_bindings[actual_index];
                        for (pattern_arg, actual_arg) in
                            pattern.trait_args.iter().zip(actual.trait_args.iter())
                        {
                            self.infer_const_generics_from_type(
                                *pattern_arg,
                                *actual_arg,
                                substitutions,
                                span,
                            );
                        }
                        for (pattern_arg, actual_arg) in pattern
                            .trait_const_args
                            .iter()
                            .zip(actual.trait_const_args.iter())
                        {
                            self.infer_const_generic_from_arg(
                                pattern_arg,
                                actual_arg.clone(),
                                substitutions,
                                span,
                            );
                        }
                        self.infer_const_generics_from_type(
                            pattern.ty,
                            actual.ty,
                            substitutions,
                            span,
                        );
                    }
                }
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }),
            ) if pattern_trait == actual_trait
                && pattern_args.len() == actual_args.len()
                && pattern_const_args.len() == actual_const_args.len()
                && pattern_bindings.len() == actual_bindings.len() =>
            {
                for (pattern, actual) in pattern_args.into_iter().zip(actual_args) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args) {
                    self.infer_const_generic_from_arg(pattern, actual, substitutions, span);
                }
                if let Some(matches) = self.infer_associated_const_bindings(
                    &pattern_bindings,
                    &actual_bindings,
                    substitutions,
                ) {
                    for (pattern, actual_index) in pattern_bindings.iter().zip(matches) {
                        let actual = &actual_bindings[actual_index];
                        for (pattern_arg, actual_arg) in
                            pattern.trait_args.iter().zip(actual.trait_args.iter())
                        {
                            self.infer_const_generics_from_type(
                                *pattern_arg,
                                *actual_arg,
                                substitutions,
                                span,
                            );
                        }
                        for (pattern_arg, actual_arg) in pattern
                            .trait_const_args
                            .iter()
                            .zip(actual.trait_const_args.iter())
                        {
                            self.infer_const_generic_from_arg(
                                pattern_arg,
                                actual_arg.clone(),
                                substitutions,
                                span,
                            );
                        }
                        self.infer_const_generics_from_type(
                            pattern.ty,
                            actual.ty,
                            substitutions,
                            span,
                        );
                    }
                }
            }
            (
                Some(TyKind::Projection {
                    self_ty: pattern_self,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    name: pattern_name,
                }),
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }),
            ) if pattern_trait == actual_trait
                && pattern_name == actual_name
                && pattern_args.len() == actual_args.len()
                && pattern_const_args.len() == actual_const_args.len() =>
            {
                self.infer_const_generics_from_type(pattern_self, actual_self, substitutions, span);
                for (pattern, actual) in pattern_args.into_iter().zip(actual_args) {
                    self.infer_const_generics_from_type(pattern, actual, substitutions, span);
                }
                for (pattern, actual) in pattern_const_args.iter().zip(actual_const_args) {
                    self.infer_const_generic_from_arg(pattern, actual, substitutions, span);
                }
            }
            _ => {}
        }
    }

    fn infer_const_generic_from_arg(
        &mut self,
        pattern: &ConstGenericArg,
        actual: ConstGenericArg,
        substitutions: &mut SymbolMap<ConstGenericArg>,
        span: Span,
    ) {
        self.infer_const_generics_from_type(pattern.ty, actual.ty, substitutions, span);
        if let ConstGenericValue::GenericParam(name) = &pattern.value {
            self.record_const_generic_substitution(name, actual, substitutions, span);
        }
    }

    fn infer_const_generic_from_array_len(
        &mut self,
        pattern: ArrayLenTy,
        actual: ArrayLenTy,
        substitutions: &mut SymbolMap<ConstGenericArg>,
        span: Span,
    ) {
        let ArrayLenTy::GenericParam(name) = pattern else {
            return;
        };
        let Some(value) = self.const_generic_value_from_array_len(actual) else {
            return;
        };
        let ty = self.primitive(PrimitiveTy::Usize);
        self.record_const_generic_substitution(
            &name,
            ConstGenericArg { ty, value },
            substitutions,
            span,
        );
    }

    fn record_const_generic_substitution(
        &mut self,
        name: &SymbolId,
        arg: ConstGenericArg,
        substitutions: &mut SymbolMap<ConstGenericArg>,
        span: Span,
    ) {
        if let Some(existing) = substitutions.get(name).cloned() {
            if !self.const_generic_args_match(&existing, &arg) {
                let name = self.symbol_name(*name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("conflicting inferred value for const generic parameter `{name}`"),
                ));
            }
        } else {
            substitutions.insert(*name, arg);
        }
    }

    fn const_generic_value_from_array_len(&self, len: ArrayLenTy) -> Option<ConstGenericValue> {
        match len {
            ArrayLenTy::ConstValue(value) => {
                Some(ConstGenericValue::Int(IntConst::unsigned(value.into())))
            }
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(id)
                .map(|value| ConstGenericValue::Int(IntConst::unsigned(value.into()))),
            ArrayLenTy::Builtin { builtin, ty } => self
                .array_len_value(Span::default(), &ArrayLenTy::Builtin { builtin, ty })
                .ok()
                .map(|value| ConstGenericValue::Int(IntConst::unsigned(value.into()))),
            ArrayLenTy::GenericParam(name) => Some(ConstGenericValue::GenericParam(name)),
            ArrayLenTy::Infer => None,
        }
    }
}

fn type_generic_names(signature: &FunctionSignature) -> Vec<SymbolId> {
    signature
        .generic_params
        .iter()
        .filter_map(|param| match param.kind {
            GenericParamSignatureKind::Type => Some(param.name),
            GenericParamSignatureKind::Const { .. } => None,
        })
        .collect()
}

pub(super) fn builtin_function(signature: &FunctionSignature) -> Option<BuiltinFunction> {
    signature
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            FunctionAttribute::Builtin(builtin) => Some(*builtin),
            FunctionAttribute::Naked => None,
        })
}
