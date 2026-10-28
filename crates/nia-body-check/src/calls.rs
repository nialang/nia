// SPDX-License-Identifier: GPL-3.0-or-later
//! Call checking for functions, methods, associated items, and builtins.
//!
//! A failed callable lookup owns the callee span, leaving independent argument
//! diagnostics visible. Ambiguous bracket syntax keeps a valid type
//! interpretation when its base is an error; value-shaped arguments are still
//! checked. Candidate reports retain declaration-owned paths, distinguish
//! ordinary parameters from receivers, and sort identities and locations
//! before applying display limits. Failed method lookup on a structural error
//! receiver checks independent arguments without falling back to a field-shape
//! error or requesting an unavailable provider; known method candidates
//! continue through ordinary signature checking.
//!
//! Required type arguments validate unresolved candidate leaves recursively
//! through lowered type sites and report the leaf span. Unused type readings
//! of const arguments are excluded. Nested bracket expressions use the same
//! candidate-aware resolution as outer expressions, and error-union candidates
//! visit both payload types.
mod args;
mod asm;
mod builtins;
mod function_calls;
mod generic_args;
mod methods;
mod signatures;

use crate::BodyChecker;
use builtins::{BuiltinCallArguments, BuiltinCallTypeArgs};
use nia_ast::{Expr, ExprKind};
use nia_ids::{BuiltinFunction, InternedTyId};
use nia_local_resolve::LocalUse;
use nia_sema_ir::BracketSuffixResolution;
use nia_symbol::known;
use nia_value_resolve::ValueNameResolution;

#[derive(Debug, Clone)]
pub(super) struct ResolvedFunctionSignature {
    pub(super) def_id: nia_ids::GlobalDefId,
    pub(super) signature: nia_item_signatures::FunctionSignature,
}

impl<'a> BodyChecker<'a> {
    /// Lookup failures belong to the callee; argument errors have independent spans.
    fn call_callee_span(expr: &Expr) -> nia_span::Span {
        match &expr.kind {
            ExprKind::Call { callee, .. } => callee.span,
            _ => expr.span,
        }
    }

    pub(crate) fn check_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        // The local constraint pass can learn a call's result from later
        // closure uses or a propagation payload. Consume it as context for
        // ordinary call inference, never as a prechecked semantic result.
        let expected = expected.or_else(|| self.inferred_call_results.get(&expr.node_key).copied());
        if let Some(builtin) = std_builtin_function(callee) {
            return self.check_builtin_function_call(
                span,
                callee.span,
                expr,
                builtin,
                None,
                BuiltinCallArguments {
                    type_args: BuiltinCallTypeArgs::Bracket(&[]),
                    args,
                },
            );
        }
        if let ExprKind::BracketSuffix {
            callee: generic_callee,
            args: type_args,
        } = &callee.kind
        {
            self.record_bracket_suffix_node_resolution(
                callee,
                BracketSuffixResolution::GenericCall,
            );
            return self.check_explicit_generic_call(
                expr,
                callee,
                generic_callee,
                type_args,
                args,
                expected,
            );
        }
        if let Some(resolved) = self.qualified_callee_signature(callee) {
            if let Some(builtin) = function_calls::builtin_function(&resolved.signature) {
                return self.check_builtin_function_call(
                    span,
                    callee.span,
                    expr,
                    builtin,
                    Some(resolved.signature.return_type),
                    BuiltinCallArguments {
                        type_args: BuiltinCallTypeArgs::Bracket(&[]),
                        args,
                    },
                );
            }
            return self.check_function_signature_call(expr, &resolved, args, expected);
        }
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_field_method_call(expr, lhs, name, args, expected)
        {
            return return_type;
        }
        if let ExprKind::Qualified { lhs, name, .. } = &callee.kind
            && let Some(return_type) = self.check_associated_call(expr, lhs, name, args, expected)
        {
            return return_type;
        }
        if let ExprKind::OmittedMember { name } = &callee.kind
            && let Some(expected) = expected
        {
            if let Some(return_type) =
                self.check_omitted_enum_variant_call(expr, name, args, expected)
            {
                return return_type;
            }
            if self.enum_global_def_id(expected).is_none()
                && let Some(return_type) = self.check_associated_call_for_target(
                    expr,
                    expected,
                    name,
                    None,
                    args,
                    Some(expected),
                )
            {
                return return_type;
            }
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            if let Some(builtin) = function_calls::builtin_function(&resolved.signature) {
                return self.check_builtin_function_call(
                    span,
                    callee.span,
                    expr,
                    builtin,
                    Some(resolved.signature.return_type),
                    BuiltinCallArguments {
                        type_args: BuiltinCallTypeArgs::Bracket(&[]),
                        args,
                    },
                );
            }
            return self.check_function_signature_call(expr, &resolved, args, expected);
        }
        if let ExprKind::Ident(name) = &callee.kind
            && matches!(
                self.value_name(callee),
                None | Some(ValueNameResolution::LocalDeferred | ValueNameResolution::Error)
            )
            && matches!(self.local_use(callee), None | Some(LocalUse::Unresolved))
            && let Some(current_def_id) = self.current_def_id
            && let Some(lookup) = self.ensure_extension_method_lookup_for_id(current_def_id)
        {
            let target_ty = lookup.target_ty;
            if let Some(return_type) =
                self.check_associated_call_for_target(expr, target_ty, name, None, args, expected)
            {
                return return_type;
            }
        }
        let callee_ty = self.check_expr(callee);
        self.check_function_pointer_call_with_callee_ty(expr, callee_ty, args)
    }
}

pub(super) fn std_builtin_function(expr: &Expr) -> Option<BuiltinFunction> {
    let ExprKind::Qualified { lhs, name, .. } = &expr.kind else {
        return None;
    };
    let ExprKind::Qualified {
        lhs: std_expr,
        name: builtin_segment,
        ..
    } = &lhs.kind
    else {
        return None;
    };
    let ExprKind::Ident(root) = &std_expr.kind else {
        return None;
    };
    (*root == known::STD && *builtin_segment == known::BUILTIN)
        .then(|| crate::symbols::builtin_function_symbol(*name))
        .flatten()
        .filter(|builtin| *builtin != BuiltinFunction::CallerLocation)
}
