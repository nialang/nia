// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
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
        if (!is_variadic && actual != expected) || (is_variadic && actual < expected) {
            let expected = if is_variadic {
                format!("at least {expected}")
            } else {
                expected.to_string()
            };
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("argument count mismatch: expected {expected}, got {actual}"),
            ));
        }
    }
}
