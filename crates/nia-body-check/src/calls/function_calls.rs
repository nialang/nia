// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BodyChecker, ResolvedFunctionSignature, generic_inst_base};
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_body_ir::{BracketSuffixResolution, FunctionReference, ResolvedCall};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::FunctionSignature;
use nia_span::Span;
use nia_ty::TyKind;
use nia_value_resolve::ValueNameResolution;

struct FunctionItemRef {
    resolved: ResolvedFunctionSignature,
    type_args: Vec<InternedTyId>,
    receiver_ty: Option<InternedTyId>,
}

impl<'a> BodyChecker<'a> {
    pub(super) fn direct_callee_signature(
        &self,
        callee: &Expr,
    ) -> Option<ResolvedFunctionSignature> {
        let base = generic_inst_base(callee);
        let ExprKind::Ident(_) = &base.kind else {
            return None;
        };
        let Some(ValueNameResolution::Def(def_id)) = self.values.names.get(&base.span) else {
            return None;
        };
        self.signatures
            .functions
            .get(def_id)
            .cloned()
            .map(|signature| ResolvedFunctionSignature {
                def_id: self.global_def_id(*def_id),
                signature,
            })
    }

    pub(crate) fn check_function_ref(
        &mut self,
        expr: &Expr,
        is_const: bool,
    ) -> Option<InternedTyId> {
        let item = self.function_item_ref(expr)?;
        if !is_const {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                "function pointers must be formed with `&const`",
            ));
            return Some(self.error());
        }
        let signature = item.resolved.signature;
        let substitutions = self.generic_substitutions_for_function_ref(
            expr.span,
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
        Some(self.interner.intern(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic: signature.is_variadic,
        }))
    }

    fn function_item_ref(&mut self, expr: &Expr) -> Option<FunctionItemRef> {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => {
                let mut item = self.function_item_ref(callee)?;
                self.record_bracket_suffix_resolution(
                    expr.span,
                    BracketSuffixResolution::GenericCall,
                );
                let type_args = self.lower_bracket_type_args(args);
                item.type_args.extend(type_args);
                Some(item)
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(item) = self.associated_method_item_ref(expr.span, lhs, name) {
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
                }),
        }
    }

    fn associated_method_item_ref(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
    ) -> Option<FunctionItemRef> {
        let (target_ty, method_id, target_substitutions) = if let ExprKind::TypeTarget { ty } =
            &ty_expr.kind
        {
            let target_ty = self.ty_for_span(ty.span);
            let candidates = self.method_candidates_for_target(target_ty, name);
            let method_id = self.single_method_candidate(span, name, candidates)?;
            let target_substitutions = self.extension_target_substitutions(method_id, target_ty);
            (Some(target_ty), method_id, target_substitutions)
        } else {
            let (struct_id, type_args) = self.type_prefix_instance(ty_expr)?;
            let candidates = self.method_candidates_for_struct(struct_id, name);
            let method_id = self.single_method_candidate(span, name, candidates)?;
            let target_ty = (!type_args.is_empty() || self.type_prefix_has_no_generics(struct_id))
                .then(|| {
                    self.check_type_prefix_arg_count(ty_expr.span, struct_id, type_args.len());
                    self.interner.intern(TyKind::Nominal {
                        def_id: struct_id,
                        args: type_args,
                    })
                });
            let target_substitutions = target_ty
                .map(|target_ty| self.extension_target_substitutions(method_id, target_ty))
                .unwrap_or_default();
            (target_ty, method_id, target_substitutions)
        };
        let resolved = self.resolved_function_signature(method_id)?;
        let receiver_ty = target_ty.and_then(|target_ty| {
            resolved
                .signature
                .params
                .first()
                .and_then(|param| param.receiver)
                .map(|receiver| self.receiver_ty_for_target(target_ty, receiver))
        });
        Some(FunctionItemRef {
            resolved,
            type_args: self.extension_target_instance_args(method_id, &target_substitutions),
            receiver_ty,
        })
    }

    fn generic_substitutions_for_function_ref(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[InternedTyId],
    ) -> Option<HashMap<String, InternedTyId>> {
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
            self.diagnostics.push(Diagnostic::error(span, message));
            return None;
        }
        if !type_args.is_empty() {
            self.record_generic_instantiation(def_id, type_args, span);
        }
        self.record_function_reference(
            span,
            FunctionReference {
                def_id,
                args: type_args.to_vec(),
            },
        );
        let mut substitutions = self.generic_substitutions(&generics, type_args);
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                self.diagnostics.push(Diagnostic::error(
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
        span: Span,
        resolved: &ResolvedFunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let signature = &resolved.signature;
        if signature.generics.is_empty() {
            let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
            self.check_direct_call_args(span, args, &params, signature.is_variadic);
            self.record_resolved_call(span, ResolvedCall::Function(resolved.def_id));
            return signature.return_type;
        }
        self.check_inferred_generic_function_call(span, resolved.def_id, signature, args, expected)
    }

    pub(super) fn check_explicit_generic_call(
        &mut self,
        span: Span,
        callee_span: Span,
        callee: &Expr,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_explicit_generic_field_method_call(
                span, lhs, name, type_args, args, expected,
            )
        {
            self.record_bracket_suffix_resolution(
                callee_span,
                BracketSuffixResolution::GenericCall,
            );
            return return_type;
        }
        if let ExprKind::Qualified { lhs, name } = &callee.kind
            && let Some(return_type) = self
                .check_explicit_generic_associated_call(span, lhs, name, type_args, args, expected)
        {
            self.record_bracket_suffix_resolution(
                callee_span,
                BracketSuffixResolution::GenericCall,
            );
            return return_type;
        }
        if let Some(resolved) = self.qualified_callee_signature(callee) {
            self.record_bracket_suffix_resolution(
                callee_span,
                BracketSuffixResolution::GenericCall,
            );
            return self.check_instantiated_function_call(
                span,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
            );
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            self.record_bracket_suffix_resolution(
                callee_span,
                BracketSuffixResolution::GenericCall,
            );
            return self.check_instantiated_function_call(
                span,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
            );
        }
        let callee_ty = self.check_bracket_suffix_expr(callee_span, callee, type_args, None);
        self.record_expr_type(callee_span, callee_ty);
        self.check_function_pointer_call_with_callee_ty(span, callee_ty, args)
    }

    pub(super) fn check_function_pointer_call_with_callee_ty(
        &mut self,
        span: Span,
        callee_ty: InternedTyId,
        args: &[Expr],
    ) -> InternedTyId {
        match self.interner.get(callee_ty).cloned() {
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                self.check_direct_call_args(span, args, &params, is_variadic);
                self.record_resolved_call(span, ResolvedCall::FunctionPointer);
                return_type
            }
            Some(TyKind::Error) | None => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.error()
            }
            _ => {
                self.diagnostics
                    .push(Diagnostic::error(span, "callee is not callable"));
                for arg in args {
                    self.check_expr(arg);
                }
                self.error()
            }
        }
    }

    fn check_instantiated_function_call(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[BracketArg],
        args: &[Expr],
    ) -> InternedTyId {
        let lowered_args = self.lower_bracket_type_args(type_args);
        if signature.generics.len() != lowered_args.len() {
            self.diagnostics.push(Diagnostic::error(
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
        self.record_generic_instantiation(def_id, &lowered_args, span);
        self.record_resolved_call(
            span,
            ResolvedCall::FunctionInstance {
                def_id,
                args: lowered_args.clone(),
            },
        );
        let substitutions = self.generic_substitutions(&signature.generics, &lowered_args);
        let params: Vec<InternedTyId> = signature
            .params
            .iter()
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        self.check_direct_call_args(span, args, &params, signature.is_variadic);
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        self.normalize_projection(return_type)
    }

    fn check_inferred_generic_function_call(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let params: Vec<InternedTyId> = signature.params.iter().map(|param| param.ty).collect();
        let mut substitutions = HashMap::new();
        if let Some(expected) = expected {
            self.infer_generics_from_type(
                signature.return_type,
                expected,
                &mut substitutions,
                span,
            );
        }
        let actuals: Vec<InternedTyId> = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if let Some(expected) = params
                    .get(index)
                    .copied()
                    .map(|param| self.substitute_generics(param, &substitutions))
                {
                    self.check_expr_with_expected(arg, Some(expected))
                } else {
                    self.check_expr(arg)
                }
            })
            .collect();
        self.check_call_arg_count(span, args.len(), params.len(), signature.is_variadic);

        for (param, (arg, actual)) in params.iter().zip(args.iter().zip(actuals.iter())) {
            self.infer_generics_from_type(*param, *actual, &mut substitutions, arg.span);
        }

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
        if !complete {
            return self.error();
        }
        let instance_args = signature
            .generics
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        self.record_generic_instantiation(def_id, &instance_args, span);
        self.record_resolved_call(
            span,
            ResolvedCall::FunctionInstance {
                def_id,
                args: instance_args,
            },
        );

        let instantiated_params: Vec<InternedTyId> = params
            .iter()
            .map(|param| self.substitute_generics(*param, &substitutions))
            .collect();
        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = instantiated_params.get(index).copied() {
                let actual = self.check_expr_with_expected(arg, Some(expected));
                self.expect_expr_type(arg, expected, actual, "call argument");
            }
        }
        let return_type = self.substitute_generics(signature.return_type, &substitutions);
        self.normalize_projection(return_type)
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
                    if !self.types_match(existing, actual) {
                        self.diagnostics.push(Diagnostic::error(
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
                is_const: pattern_const,
                elem: pattern_elem,
            }) => {
                if let Some(TyKind::Pointer {
                    is_const: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && pattern_const == actual_const
                {
                    self.infer_generics_from_type(pattern_elem, actual_elem, substitutions, span);
                }
            }
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => {
                if let Some(TyKind::Slice {
                    is_const: actual_const,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                    && pattern_const == actual_const
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
                is_const: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                associated_type_bindings: pattern_bindings,
            }) => {
                if let Some(TyKind::TraitObject {
                    is_const: actual_const,
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
            Some(TyKind::Error | TyKind::Primitive(_)) | None => {}
        }
    }
}
