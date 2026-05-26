// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind, ReceiverKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_item_signatures::FunctionSignature;
use nia_span::Span;
use nia_ty::TyKind;

struct MethodCall<'a> {
    span: Span,
    receiver: &'a Expr,
    receiver_ty: TyId,
    name: &'a str,
    type_args: Option<&'a [BracketArg]>,
    args: &'a [Expr],
    expected: Option<TyId>,
}

struct MethodGenericContext<'a> {
    span: Span,
    receiver_ty: TyId,
    method_id: GlobalDefId,
    method_args: Option<&'a [BracketArg]>,
    lowered_method_args: &'a [TyId],
    expected: Option<TyId>,
}

impl<'a> BodyChecker<'a> {
    pub(super) fn check_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        args: &[Expr],
        expected: Option<TyId>,
    ) -> Option<TyId> {
        self.check_associated_call_inner(span, ty_expr, name, None, args, expected)
    }

    pub(super) fn check_explicit_generic_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        method_type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<TyId>,
    ) -> Option<TyId> {
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
        expected: Option<TyId>,
    ) -> Option<TyId> {
        let (struct_id, mut type_args) = self.type_prefix_instance(ty_expr)?;
        let candidates = self.method_defs_for_struct(struct_id, name);
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
        if type_args.is_empty()
            && let Some(expected) = expected
            && let Some(inferred) = self
                .infer_associated_type_args_from_expected_return(struct_id, &signature, expected)
        {
            type_args = inferred;
        }
        if !self.check_type_prefix_arg_count(span, struct_id, type_args.len()) {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let substitutions = self.struct_generic_substitutions(struct_id, &type_args);
        let Some(method_instantiation_args) = self.lowered_method_type_args(method_type_args)
        else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let mut substitutions = substitutions;
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
            let receiver_ty = self.receiver_ty_for_struct(struct_id, &type_args, receiver_kind);
            let actual = self.check_expr_with_expected(first_arg, Some(receiver_ty));
            self.expect_expr_type(first_arg, receiver_ty, actual, "receiver argument");
        }
        let params: Vec<TyId> = signature
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
        let params: Vec<TyId> = signature
            .params
            .iter()
            .skip(if is_receiver_method { 1 } else { 0 })
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        self.check_direct_call_args(span, value_args, &params, false);
        if !type_args.is_empty() || !method_instantiation_args.is_empty() {
            let mut instance_args = type_args.clone();
            instance_args.extend(method_instantiation_args);
            self.record_generic_instantiation(method_id, &instance_args, span);
        }
        Some(self.substitute_generics(signature.return_type, &substitutions))
    }

    fn receiver_ty_for_struct(
        &mut self,
        def_id: GlobalDefId,
        args: &[TyId],
        receiver: ReceiverKind,
    ) -> TyId {
        let nominal = self.interner.intern(nia_ty::TyKind::Nominal {
            def_id,
            args: args.to_vec(),
        });
        match receiver {
            ReceiverKind::Value => nominal,
            ReceiverKind::RefConst => self.interner.intern(nia_ty::TyKind::Pointer {
                is_const: true,
                elem: nominal,
            }),
            ReceiverKind::Ref => self.interner.intern(nia_ty::TyKind::Pointer {
                is_const: false,
                elem: nominal,
            }),
        }
    }

    fn infer_associated_type_args_from_expected_return(
        &mut self,
        struct_id: GlobalDefId,
        signature: &FunctionSignature,
        expected: TyId,
    ) -> Option<Vec<TyId>> {
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

    fn check_type_prefix_arg_count(
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

    pub(crate) fn type_prefix_instance(&mut self, expr: &Expr) -> Option<(GlobalDefId, Vec<TyId>)> {
        if let ExprKind::BracketSuffix { callee, args } = &expr.kind {
            let def_id = self.type_prefix_def_id(callee)?;
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
        expected: Option<TyId>,
    ) -> Option<TyId> {
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
        expected: Option<TyId>,
    ) -> Option<TyId> {
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

    fn check_method_call_with_receiver_ty(&mut self, call: MethodCall<'_>) -> Option<TyId> {
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

        let receiver_kind = receiver_param
            .receiver
            .expect("receiver param was checked above");
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
        let params: Vec<TyId> = signature
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
        }
        Some(self.substitute_generics(signature.return_type, &substitutions))
    }

    pub(crate) fn method_defs_for_struct(
        &mut self,
        struct_id: GlobalDefId,
        name: &str,
    ) -> Vec<GlobalDefId> {
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(target_ty, method_id)| {
                let target_ty = self.normalization.normalize(target_ty);
                matches!(
                    self.interner.get(target_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == struct_id
                )
                .then_some(method_id)
            })
            .collect()
    }

    fn method_candidates_for_receiver(
        &mut self,
        receiver_ty: TyId,
        name: &str,
    ) -> Vec<GlobalDefId> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(target_ty, method_id)| {
                self.match_receiver_target(target_ty, receiver_ty, &mut HashMap::new())
                    .then_some(method_id)
            })
            .collect()
    }

    fn single_method_candidate(
        &mut self,
        span: Span,
        name: &str,
        candidates: Vec<GlobalDefId>,
    ) -> Option<GlobalDefId> {
        match candidates.as_slice() {
            [method] => Some(*method),
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

    fn lowered_method_type_args(&mut self, type_args: Option<&[BracketArg]>) -> Option<Vec<TyId>> {
        type_args
            .map(|args| self.lower_bracket_type_args(args))
            .or(Some(Vec::new()))
    }

    fn method_generic_substitutions(
        &mut self,
        context: MethodGenericContext<'_>,
        signature: &FunctionSignature,
    ) -> Option<HashMap<String, TyId>> {
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

    fn extension_target_substitutions(
        &mut self,
        method_id: GlobalDefId,
        receiver_ty: TyId,
    ) -> HashMap<String, TyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return HashMap::new();
        };
        let mut substitutions = HashMap::new();
        self.match_receiver_target(target_ty, receiver_ty, &mut substitutions);
        substitutions
    }

    fn extension_target_instance_args(
        &mut self,
        method_id: GlobalDefId,
        substitutions: &HashMap<String, TyId>,
    ) -> Vec<TyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return Vec::new();
        };
        self.generic_params_in_ty(target_ty)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn extension_target_ty_for_method(&self, method_id: GlobalDefId) -> Option<TyId> {
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
        target_ty: TyId,
        receiver_ty: TyId,
        substitutions: &mut HashMap<String, TyId>,
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

    fn match_type_pattern(
        &self,
        pattern: TyId,
        actual: TyId,
        substitutions: &mut HashMap<String, TyId>,
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
                Some(TyKind::Array { len, elem }) if pattern_len == len => {
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

    fn infer_method_generics_from_args(
        &mut self,
        args: &[Expr],
        params: &[TyId],
        substitutions: &mut HashMap<String, TyId>,
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
        substitutions: &HashMap<String, TyId>,
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
        receiver_ty: TyId,
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
