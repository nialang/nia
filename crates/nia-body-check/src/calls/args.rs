// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::InternedTyId;
use nia_sema::{ArityCheck, ArityRequirement, check_call_arity};
use nia_span::Span;

impl<'a> BodyChecker<'a> {
    pub(super) fn check_direct_call_args(
        &mut self,
        span: Span,
        args: &[Expr],
        params: &[InternedTyId],
        is_variadic: bool,
    ) {
        self.check_call_arg_count(span, args.len(), params.len(), is_variadic);

        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = params.get(index).copied() {
                let arg_ty = self.check_expr_with_expected(arg, Some(expected));
                self.expect_expr_type(arg, expected, arg_ty, "call argument");
            } else {
                self.check_expr(arg);
            }
        }
    }

    pub(super) fn check_call_arg_count(
        &mut self,
        span: Span,
        actual: usize,
        expected: usize,
        is_variadic: bool,
    ) {
        let ArityCheck::Mismatch {
            requirement,
            actual,
        } = check_call_arity(expected, actual, is_variadic)
        else {
            return;
        };
        let expected = match requirement {
            ArityRequirement::Exact(expected) => expected.to_string(),
            ArityRequirement::AtLeast(expected) => format!("at least {expected}"),
        };
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("argument count mismatch: expected {expected}, got {actual}"),
        ));
    }
}
