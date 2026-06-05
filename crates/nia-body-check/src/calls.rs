// SPDX-License-Identifier: GPL-3.0-or-later
mod args;
mod asm;
mod builtins;
mod function_calls;
mod generic_args;
mod methods;
mod signature_import;

use crate::BodyChecker;
use nia_ast::{Expr, ExprKind};
use nia_ids::InternedTyId;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let span = expr.span;
        if let ExprKind::Builtin { name, type_arg } = &callee.kind {
            return self.check_builtin_call(span, callee, name, type_arg, args);
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
        let callee_ty = self.check_expr(callee);
        self.check_function_pointer_call_with_callee_ty(expr, callee_ty, args)
    }
}
