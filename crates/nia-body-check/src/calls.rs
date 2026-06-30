// SPDX-License-Identifier: GPL-3.0-or-later
mod args;
mod asm;
mod builtins;
mod function_calls;
mod generic_args;
mod methods;
mod signature_import;

use crate::BodyChecker;
use builtins::BuiltinCallTypeArgs;
use nia_ast::{Expr, ExprKind};
use nia_ids::BuiltinFunction;
use nia_ids::InternedTyId;
use nia_local_resolve::LocalUse;
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        if let Some(builtin) = std_builtin_function(callee) {
            return self.check_builtin_function_call(
                span,
                callee.span,
                expr,
                builtin,
                BuiltinCallTypeArgs::Bracket(&[]),
                args,
            );
        }
        if let ExprKind::BracketSuffix {
            callee: generic_callee,
            args: type_args,
        } = &callee.kind
        {
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
            return self.check_function_signature_call(expr, &resolved, args, expected);
        }
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_field_method_call(expr, lhs, name, args, expected)
        {
            return return_type;
        }
        if let ExprKind::Qualified { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_associated_call(expr, lhs, name, args, expected)
        {
            return return_type;
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            return self.check_function_signature_call(expr, &resolved, args, expected);
        }
        if let ExprKind::Ident(name) = &callee.kind
            && matches!(
                self.value_name(callee),
                None | Some(ValueNameResolution::LocalDeferred | ValueNameResolution::Error)
            )
            && matches!(self.local_use(callee), None | Some(LocalUse::Unresolved))
            && let Some(current_def_id) = self.current_def_id
            && let Some(lookup) = self.extension_method_lookup_for_id(current_def_id)
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
    let ExprKind::Qualified { lhs, name } = &expr.kind else {
        return None;
    };
    let ExprKind::Qualified {
        lhs: std_expr,
        name: builtin_segment,
    } = &lhs.kind
    else {
        return None;
    };
    let ExprKind::Ident(root) = &std_expr.kind else {
        return None;
    };
    (root == "std" && builtin_segment == "builtin")
        .then(|| BuiltinFunction::from_name(name))
        .flatten()
}
