// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BodyChecker, ReceiverBase};
use nia_ast::{BracketArg, Expr, ExprKind, ReceiverKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_item_signatures::FunctionSignature;
use nia_span::Span;

impl<'a> BodyChecker<'a> {
    pub(super) fn check_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        args: &[Expr],
    ) -> Option<TyId> {
        self.check_associated_call_inner(span, ty_expr, name, None, args)
    }

    pub(super) fn check_explicit_generic_associated_call(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        method_type_args: &[BracketArg],
        args: &[Expr],
    ) -> Option<TyId> {
        self.check_associated_call_inner(span, ty_expr, name, Some(method_type_args), args)
    }

    fn check_associated_call_inner(
        &mut self,
        span: Span,
        ty_expr: &Expr,
        name: &str,
        method_type_args: Option<&[BracketArg]>,
        args: &[Expr],
    ) -> Option<TyId> {
        let (struct_id, type_args) = self.type_prefix_instance(ty_expr)?;
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
        if signature.generics.len() != method_arg_count
            && (!signature.generics.is_empty() || method_type_args.is_some())
        {
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
    ) -> Option<TyId> {
        let receiver_ty = self.check_expr(receiver);
        let base = self.receiver_base_type(receiver_ty)?;
        let candidates = self.method_defs_for_struct(base.def_id, name);
        if candidates.is_empty() {
            return None;
        }
        self.single_method_candidate(span, name, candidates)?;
        self.check_method_call_with_receiver_ty(span, receiver, receiver_ty, name, None, args)
    }

    pub(super) fn check_explicit_generic_field_method_call(
        &mut self,
        span: Span,
        receiver: &Expr,
        name: &str,
        type_args: &[BracketArg],
        args: &[Expr],
    ) -> Option<TyId> {
        let receiver_ty = self.check_expr(receiver);
        let base = self.receiver_base_type(receiver_ty)?;
        let candidates = self.method_defs_for_struct(base.def_id, name);
        if candidates.is_empty() {
            return None;
        }
        self.single_method_candidate(span, name, candidates)?;
        self.check_method_call_with_receiver_ty(
            span,
            receiver,
            receiver_ty,
            name,
            Some(type_args),
            args,
        )
    }

    fn check_method_call_with_receiver_ty(
        &mut self,
        span: Span,
        receiver: &Expr,
        receiver_ty: TyId,
        name: &str,
        type_args: Option<&[BracketArg]>,
        args: &[Expr],
    ) -> Option<TyId> {
        let base = self.receiver_base_type(receiver_ty)?;
        let method_id = self.single_method_candidate(
            span,
            name,
            self.method_defs_for_struct(base.def_id, name),
        )?;
        let Some(signature) = self
            .resolved_function_signature(method_id)
            .map(|resolved| resolved.signature)
        else {
            self.diagnostics
                .push(Diagnostic::error(span, "method signature not found"));
            return Some(self.error());
        };
        let Some(receiver_param) = signature
            .params
            .first()
            .filter(|param| param.receiver.is_some())
        else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "associated functions are not supported by receiver method call syntax",
            ));
            return Some(self.error());
        };

        let receiver_kind = receiver_param
            .receiver
            .expect("receiver param was checked above");
        self.check_receiver_match(receiver, &base, receiver_kind);

        let Some(method_instantiation_args) = self.lowered_method_type_args(type_args) else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let Some(substitutions) = self.method_generic_substitutions(
            span,
            base.def_id,
            &base.args,
            &signature,
            type_args,
            &method_instantiation_args,
        ) else {
            for arg in args {
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
        self.check_direct_call_args(span, args, &params, false);
        if !base.args.is_empty() || !method_instantiation_args.is_empty() {
            let mut instance_args = base.args.clone();
            instance_args.extend(method_instantiation_args);
            self.record_generic_instantiation(method_id, &instance_args, span);
        }
        Some(self.substitute_generics(signature.return_type, &substitutions))
    }

    pub(crate) fn method_defs_for_struct(
        &self,
        struct_id: GlobalDefId,
        name: &str,
    ) -> Vec<GlobalDefId> {
        self.extensions.methods(struct_id, name)
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
        span: Span,
        struct_id: GlobalDefId,
        struct_args: &[TyId],
        signature: &FunctionSignature,
        method_args: Option<&[BracketArg]>,
        lowered_method_args: &[TyId],
    ) -> Option<HashMap<String, TyId>> {
        let mut substitutions = self.struct_generic_substitutions(struct_id, struct_args);
        let method_arg_count = lowered_method_args.len();
        if signature.generics.len() != method_arg_count
            && (!signature.generics.is_empty() || method_args.is_some())
        {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "generic argument count mismatch for method: expected {}, got {method_arg_count}",
                    signature.generics.len()
                ),
            ));
            return None;
        }
        if method_args.is_some() {
            substitutions
                .extend(self.generic_substitutions(&signature.generics, lowered_method_args));
        }
        Some(substitutions)
    }

    fn check_receiver_match(
        &mut self,
        receiver: &Expr,
        base: &ReceiverBase,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind == ReceiverKind::Ref {
            if base.has_readonly_pointer {
                self.diagnostics.push(Diagnostic::error(
                    receiver.span,
                    "receiver cannot be matched through `&const T`",
                ));
            } else if !base.from_pointer {
                self.check_assignable(receiver, "receiver");
            }
        }
    }
}
