// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BodyChecker, ResolvedFunctionSignature, generic_inst_base};
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_item_signatures::FunctionSignature;
use nia_span::Span;
use nia_ty::TyKind;
use nia_value_resolve::ValueNameResolution;

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

    pub(crate) fn check_function_ref(&mut self, expr: &Expr, is_const: bool) -> Option<TyId> {
        let (resolved, type_args) = self.function_item_resolution(expr)?;
        if !is_const {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                "function pointers must be formed with `&const`",
            ));
            return Some(self.error());
        }
        let signature = resolved.signature;
        let (params, return_type, is_variadic) = if let Some(type_args) = type_args {
            let lowered_args = self.lower_bracket_type_args(&type_args);
            if signature.generics.len() != lowered_args.len() {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    format!(
                        "generic argument count mismatch for function pointer: expected {}, got {}",
                        signature.generics.len(),
                        lowered_args.len()
                    ),
                ));
                return Some(self.error());
            }
            self.record_generic_instantiation(resolved.def_id, &lowered_args, expr.span);
            let substitutions = self.generic_substitutions(&signature.generics, &lowered_args);
            (
                signature
                    .params
                    .iter()
                    .map(|param| self.substitute_generics(param.ty, &substitutions))
                    .collect(),
                self.substitute_generics(signature.return_type, &substitutions),
                signature.is_variadic,
            )
        } else {
            if !signature.generics.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "generic function pointer requires explicit type arguments",
                ));
                return Some(self.error());
            }
            (
                signature.params.iter().map(|param| param.ty).collect(),
                signature.return_type,
                signature.is_variadic,
            )
        };
        Some(self.interner.intern(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }))
    }

    fn function_item_resolution(
        &mut self,
        expr: &Expr,
    ) -> Option<(ResolvedFunctionSignature, Option<Vec<BracketArg>>)> {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => self
                .function_item_resolution(callee)
                .map(|(resolved, _)| (resolved, Some(args.clone()))),
            _ => self
                .qualified_callee_signature(expr)
                .or_else(|| self.direct_callee_signature(expr))
                .map(|resolved| (resolved, None)),
        }
    }

    pub(super) fn check_function_signature_call(
        &mut self,
        span: Span,
        resolved: &ResolvedFunctionSignature,
        args: &[Expr],
        expected: Option<TyId>,
    ) -> TyId {
        let signature = &resolved.signature;
        if signature.generics.is_empty() {
            let params: Vec<TyId> = signature.params.iter().map(|param| param.ty).collect();
            self.check_direct_call_args(span, args, &params, signature.is_variadic);
            return signature.return_type;
        }
        self.check_inferred_generic_function_call(span, resolved.def_id, signature, args, expected)
    }

    pub(super) fn check_explicit_generic_call(
        &mut self,
        span: Span,
        callee: &Expr,
        type_args: &[BracketArg],
        args: &[Expr],
        expected: Option<TyId>,
    ) -> TyId {
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_explicit_generic_field_method_call(
                span, lhs, name, type_args, args, expected,
            )
        {
            return return_type;
        }
        if let ExprKind::Qualified { lhs, name } = &callee.kind
            && let Some(return_type) = self
                .check_explicit_generic_associated_call(span, lhs, name, type_args, args, expected)
        {
            return return_type;
        }
        if let Some(resolved) = self.qualified_callee_signature(callee) {
            return self.check_instantiated_function_call(
                span,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
            );
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            return self.check_instantiated_function_call(
                span,
                resolved.def_id,
                &resolved.signature,
                type_args,
                args,
            );
        }
        self.diagnostics.push(Diagnostic::error(
            callee.span,
            "explicit generic instantiation requires a function callee",
        ));
        for arg in args {
            self.check_expr(arg);
        }
        self.error()
    }

    fn check_instantiated_function_call(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        type_args: &[BracketArg],
        args: &[Expr],
    ) -> TyId {
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
        let substitutions = self.generic_substitutions(&signature.generics, &lowered_args);
        let params: Vec<TyId> = signature
            .params
            .iter()
            .map(|param| self.substitute_generics(param.ty, &substitutions))
            .collect();
        self.check_direct_call_args(span, args, &params, signature.is_variadic);
        self.substitute_generics(signature.return_type, &substitutions)
    }

    fn check_inferred_generic_function_call(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
        args: &[Expr],
        expected: Option<TyId>,
    ) -> TyId {
        let params: Vec<TyId> = signature.params.iter().map(|param| param.ty).collect();
        let mut substitutions = HashMap::new();
        if let Some(expected) = expected {
            self.infer_generics_from_type(
                signature.return_type,
                expected,
                &mut substitutions,
                span,
            );
        }
        let actuals: Vec<TyId> = args
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

        let instantiated_params: Vec<TyId> = params
            .iter()
            .map(|param| self.substitute_generics(*param, &substitutions))
            .collect();
        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = instantiated_params.get(index).copied() {
                let actual = self.check_expr_with_expected(arg, Some(expected));
                self.expect_expr_type(arg, expected, actual, "call argument");
            }
        }
        self.substitute_generics(signature.return_type, &substitutions)
    }

    pub(crate) fn infer_generics_from_type(
        &mut self,
        pattern: TyId,
        actual: TyId,
        substitutions: &mut HashMap<String, TyId>,
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
            Some(TyKind::Error | TyKind::Primitive(_)) | None => {}
        }
    }
}
