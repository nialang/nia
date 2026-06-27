// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BodyChecker, ResolvedFunctionSignature, generic_inst_base};
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::FunctionSignature;
use nia_local_resolve::LocalUse;
use nia_sema_ir::{BracketSuffixResolution, FunctionReference, ResolvedCall};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};
use nia_value_resolve::ValueNameResolution;

struct FunctionItemRef {
    resolved: ResolvedFunctionSignature,
    type_args: Vec<InternedTyId>,
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
                signature: self.import_local_function_signature(&signature),
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
        let target_ty = self.extension_methods_by_id.get(&current_def_id)?.target_ty;
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
        Some(FunctionItemRef {
            resolved,
            type_args: self
                .extension_target_instance_args(method_id, &candidate.target_substitutions),
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
        if !is_readonly {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "function pointers must be formed with `&fn(...)`",
            ));
            return Some(self.error());
        }
        let signature = item.resolved.signature;
        let substitutions = self.generic_substitutions_for_function_ref(
            expr,
            item.resolved.def_id,
            &signature,
            &item.type_args,
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
                    self.substitute_generics(param.ty, &substitutions)
                }
            })
            .collect();
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
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
                let type_args = self.lower_bracket_type_args(args);
                item.type_args.extend(type_args);
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
                        receiver_ty: None,
                    })
            }
            _ => self
                .qualified_callee_signature(expr)
                .or_else(|| self.direct_callee_signature(expr))
                .map(|resolved| FunctionItemRef {
                    resolved,
                    type_args: Vec::new(),
                    receiver_ty: None,
                })
                .or_else(|| self.current_extension_method_callee_signature(expr)),
        }
    }

    fn associated_method_item_ref(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
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
        Some(FunctionItemRef {
            resolved,
            type_args: self
                .extension_target_instance_args(method_id, &candidate.target_substitutions),
            receiver_ty,
        })
    }

    fn generic_substitutions_for_function_ref(
        &mut self,
        expr: &Expr,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[InternedTyId],
    ) -> Option<HashMap<String, InternedTyId>> {
        let span = expr.span;
        let generics = self.effective_generics_for_def(def_id);
        if generics.len() != type_args.len() {
            let message = if type_args.is_empty() {
                "generic function pointer requires explicit type arguments".to_string()
            } else {
                format!(
                    "generic argument count mismatch for function pointer: expected {}, got {}",
                    generics.len(),
                    type_args.len()
                )
            };
            self.diagnostics
                .push(Diagnostic::user_error_at(codes::TYPE_CHECK, span, message));
            return None;
        }
        if !type_args.is_empty() {
            self.record_generic_instantiation(def_id, type_args, span);
        }
        self.record_function_node_reference(
            span,
            &expr.node_key,
            FunctionReference {
                def_id,
                arg_module_id: self.defs.module_id,
                args: type_args.to_vec(),
            },
        );
        let mut substitutions = self.generic_substitutions(&generics, type_args);
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("generic function pointer requires `{generic}`"),
                ));
                return None;
            }
        }
        Some(std::mem::take(&mut substitutions))
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
        if signature.is_comptime && !self.in_comptime_context() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "`comptime fn` can only be called from a comptime expression",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        }
        if signature.generics.is_empty() {
            let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
            self.check_direct_call_args(span, args, &params, signature.is_variadic);
            if !signature.is_comptime {
                self.record_resolved_node_call(
                    span,
                    &expr.node_key,
                    ResolvedCall::Function(resolved.def_id),
                );
            }
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
        let lowered_args = self.lower_bracket_type_args(type_args);
        if lowered_args.len() > signature.generics.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "generic argument count mismatch for function: expected {}, got {}",
                    signature.generics.len(),
                    lowered_args.len()
                ),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        }
        let mut substitutions =
            self.generic_substitutions(&signature.generics[..lowered_args.len()], &lowered_args);
        self.infer_generic_function_call_substitutions(
            span,
            signature,
            args,
            expected,
            &mut substitutions,
        );
        let Some(instance_args) =
            self.complete_generic_function_instance_args(span, &signature.generics, &substitutions)
        else {
            return self.error();
        };
        self.record_generic_instantiation(def_id, &instance_args, span);
        self.record_resolved_node_call(
            span,
            &expr.node_key,
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: self.defs.module_id,
                args: instance_args,
            },
        );
        self.check_instantiated_generic_function_call_args(span, signature, args, &substitutions);
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
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
        let mut substitutions = HashMap::new();
        self.infer_generic_function_call_substitutions(
            span,
            signature,
            args,
            expected,
            &mut substitutions,
        );
        let Some(instance_args) =
            self.complete_generic_function_instance_args(span, &signature.generics, &substitutions)
        else {
            return self.error();
        };
        self.record_generic_instantiation(def_id, &instance_args, span);
        self.record_resolved_node_call(
            span,
            &expr.node_key,
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: self.defs.module_id,
                args: instance_args,
            },
        );

        self.check_instantiated_generic_function_call_args(span, signature, args, &substitutions);
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        let return_type = self.normalize_projection(return_type);
        self.normalize_aliases_in_type(return_type)
    }

    fn infer_generic_function_call_substitutions(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
        substitutions: &mut HashMap<String, InternedTyId>,
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
            let substituted_param = self.substitute_generics(param, substitutions);
            let expected = self.generic_call_expected(substituted_param);
            let actual = if let Some(expected) = expected {
                self.check_expr_with_expected(arg, Some(expected))
            } else {
                self.check_expr(arg)
            };
            self.infer_generics_from_type(param, actual, substitutions, arg.span);
            self.infer_generic_function_call_substitutions_from_where_predicates(
                signature,
                args,
                substitutions,
            );
        }
    }

    fn infer_generic_function_call_substitutions_from_where_predicates(
        &mut self,
        signature: &FunctionSignature,
        args: &[Expr],
        substitutions: &mut HashMap<String, InternedTyId>,
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
                        substitutions.insert(generic.clone(), *ty);
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
        candidate: &HashMap<String, InternedTyId>,
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
                ExprKind::String(_),
                Some(TyKind::Slice {
                    is_readonly: true,
                    elem,
                }),
            ) => self.types_match(elem, self.primitive(PrimitiveTy::Char)),
            (
                ExprKind::ByteString(_),
                Some(TyKind::Slice {
                    is_readonly: true,
                    elem,
                }),
            ) => self.types_match(elem, self.primitive(PrimitiveTy::U8)),
            (ExprKind::String(_), _) | (ExprKind::ByteString(_), _) => false,
            _ => true,
        }
    }

    fn complete_generic_function_instance_args(
        &mut self,
        span: Span,
        generics: &[String],
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<Vec<InternedTyId>> {
        self.complete_instance_args_for_generics(span, generics, substitutions)
    }

    fn check_instantiated_generic_function_call_args(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        args: &[Expr],
        substitutions: &HashMap<String, InternedTyId>,
    ) {
        let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
        let instantiated_params: Vec<InternedTyId> = params
            .iter()
            .map(|param| self.substitute_generics(*param, substitutions))
            .collect();
        self.check_where_predicates_hold(&signature.where_predicates, substitutions, span);
        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = instantiated_params.get(index).copied() {
                let actual = self.check_expr_with_expected(arg, Some(expected));
                self.expect_expr_type(arg, expected, actual, "call argument");
            }
        }
    }

    fn generic_call_expected(&self, ty: InternedTyId) -> Option<InternedTyId> {
        if self.type_contains_generic_param(ty) {
            None
        } else {
            Some(ty)
        }
    }

    pub(crate) fn type_contains_generic_param(&self, ty: InternedTyId) -> bool {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Array { elem, .. })
            | Some(TyKind::Optional { elem }) => self.type_contains_generic_param(*elem),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_contains_generic_param(bound))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .iter()
                    .any(|param| self.type_contains_generic_param(*param))
                    || self.type_contains_generic_param(*return_type)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_contains_generic_param(*error) || self.type_contains_generic_param(*value)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.type_contains_generic_param(*arg)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.type_contains_generic_param(*arg))
                    || associated_type_bindings
                        .iter()
                        .any(|binding| self.type_contains_generic_param(binding.ty))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.type_contains_generic_param(*self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.type_contains_generic_param(*arg))
            }
            Some(
                TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => false,
        }
    }

    pub(crate) fn infer_generics_from_type(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
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
                if let Some(TyKind::Slice {
                    is_readonly: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && (pattern_const == actual_const || pattern_const && !actual_const)
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
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
                elem: pattern_elem, ..
            }) => {
                if let Some(TyKind::Array {
                    elem: actual_elem, ..
                }) = self.interner.get(actual).cloned()
                {
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
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => {
                if let Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                }) = self.interner.get(actual).cloned()
                    && pattern_def == actual_def
                    && pattern_args.len() == actual_args.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
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
                associated_type_bindings: pattern_bindings,
            }) => {
                if let Some(TyKind::TraitObject {
                    is_readonly: actual_const,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    associated_type_bindings: actual_bindings,
                }) = self.interner.get(actual).cloned()
                    && pattern_const == actual_const
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && pattern_bindings.len() == actual_bindings.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for pattern_binding in pattern_bindings {
                        if let Some(actual_binding) =
                            actual_bindings.iter().find(|actual_binding| {
                                self.associated_type_binding_keys_match(
                                    &pattern_binding,
                                    actual_binding,
                                )
                            })
                        {
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
                associated_type_bindings: pattern_bindings,
            }) => {
                if let Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    associated_type_bindings: actual_bindings,
                }) = self.interner.get(actual).cloned()
                    && pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && pattern_bindings.len() == actual_bindings.len()
                {
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                    for pattern_binding in pattern_bindings {
                        if let Some(actual_binding) =
                            actual_bindings.iter().find(|actual_binding| {
                                self.associated_type_binding_keys_match(
                                    &pattern_binding,
                                    actual_binding,
                                )
                            })
                        {
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
                name: pattern_name,
            }) => {
                if let Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    name: actual_name,
                }) = self.interner.get(actual).cloned()
                    && pattern_trait == actual_trait
                    && pattern_name == actual_name
                    && pattern_args.len() == actual_args.len()
                {
                    self.infer_generics_from_type(pattern_self, actual_self, substitutions, span);
                    for (pattern, actual) in pattern_args.iter().zip(actual_args.iter()) {
                        self.infer_generics_from_type(*pattern, *actual, substitutions, span);
                    }
                }
            }
            Some(
                TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => {}
        }
    }

    fn generic_substitution_is_self_mapping(&self, name: &str, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::GenericParam(existing)) if existing == name
        )
    }
}
