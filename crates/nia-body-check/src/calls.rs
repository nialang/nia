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
use nia_diagnostic::Diagnostic;
use nia_ids::TyId;
use nia_span::Span;
use nia_ty::TyKind;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_call(&mut self, span: Span, callee: &Expr, args: &[Expr]) -> TyId {
        if let ExprKind::Builtin { name, type_arg } = &callee.kind {
            return self.check_builtin_call(span, callee.span, name, type_arg, args);
        }
        if let ExprKind::BracketSuffix {
            callee: generic_callee,
            args: type_args,
        } = &callee.kind
        {
            return self.check_explicit_generic_call(span, generic_callee, type_args, args);
        }
        if let Some(resolved) = self.qualified_callee_signature(callee) {
            return self.check_function_signature_call(span, &resolved, args);
        }
        if let ExprKind::Field { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_field_method_call(span, lhs, name, args)
        {
            return return_type;
        }
        if let ExprKind::Qualified { lhs, name } = &callee.kind
            && let Some(return_type) = self.check_associated_call(span, lhs, name, args)
        {
            return return_type;
        }
        if let Some(resolved) = self.direct_callee_signature(callee) {
            return self.check_function_signature_call(span, &resolved, args);
        }
        let callee_ty = self.check_expr(callee);
        match self.interner.get(callee_ty).cloned() {
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                self.check_direct_call_args(span, args, &params, is_variadic);
                return_type
            }
            Some(TyKind::Error) | None => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.error()
            }
            _ => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics
                    .push(Diagnostic::error(callee.span, "callee is not a function"));
                self.error()
            }
        }
    }
}
