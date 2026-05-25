// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{ArrayElements, Expr, ExprKind};
use nia_diagnostic::Diagnostic;
use nia_ids::TyId;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

impl<'a> BodyChecker<'a> {
    pub(super) fn check_asm_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        args: &[Expr],
    ) -> TyId {
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                call_span,
                "builtin `@asm` requires exactly one configuration argument",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.void();
        }
        let config = &args[0];
        let ExprKind::StructLiteral { fields } = &config.kind else {
            self.diagnostics.push(Diagnostic::error(
                config.span,
                "builtin `@asm` expects an untyped struct literal configuration",
            ));
            self.check_expr(config);
            return self.void();
        };

        let mut has_code = false;
        for field in fields {
            match field.name.as_str() {
                "code" => {
                    has_code = true;
                    if !matches!(field.value.kind, ExprKind::String(_)) {
                        self.diagnostics.push(Diagnostic::error(
                            field.value.span,
                            "`@asm` field `code` must be a string literal",
                        ));
                    }
                }
                "inputs" => self.check_asm_inputs(&field.value),
                "outputs" => self.check_asm_outputs(&field.value),
                "clobbers" => self.check_asm_clobbers(&field.value),
                "options" => self.check_asm_options(&field.value),
                _ => {
                    self.diagnostics.push(Diagnostic::error(
                        field.span,
                        format!("unknown `@asm` field `{}`", field.name),
                    ));
                    self.check_expr(&field.value);
                }
            }
        }
        if !has_code {
            self.diagnostics.push(Diagnostic::error(
                builtin_span,
                "builtin `@asm` requires a `code` string literal",
            ));
        }
        self.void()
    }

    fn check_asm_inputs(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                "`@asm` field `inputs` must be a struct literal",
            ));
            self.check_expr(expr);
            return;
        };
        for field in fields {
            let value_ty = self.check_expr(&field.value);
            self.check_asm_operand_type(field.value.span, value_ty, "inline assembly input");
        }
    }

    fn check_asm_outputs(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                "`@asm` field `outputs` must be a struct literal",
            ));
            self.check_expr(expr);
            return;
        };
        for field in fields {
            let value_ty = self.check_expr(&field.value);
            self.check_assignable(&field.value, "inline assembly output");
            self.check_asm_operand_type(field.value.span, value_ty, "inline assembly output");
        }
    }

    fn check_asm_operand_type(&mut self, span: Span, ty: TyId, context: &str) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never)) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("{context} cannot have void or never type"),
                ));
            }
            Some(TyKind::Array { .. } | TyKind::Slice { .. }) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("{context} cannot use aggregate type directly"),
                ));
            }
            Some(TyKind::Nominal { def_id, .. }) if !self.is_enum_def(*def_id) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("{context} cannot use aggregate type directly"),
                ));
            }
            Some(
                TyKind::Primitive(_)
                | TyKind::Pointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Nominal { .. },
            ) => {}
            Some(TyKind::GenericParam(_) | TyKind::Error) | None => {}
        }
    }

    fn check_asm_clobbers(&mut self, expr: &Expr) {
        let ExprKind::ArrayLiteral {
            elems: ArrayElements::List(elems),
        } = &expr.kind
        else {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                "`@asm` field `clobbers` must be an array literal of strings",
            ));
            self.check_expr(expr);
            return;
        };
        for elem in elems {
            if !matches!(elem.kind, ExprKind::String(_)) {
                self.diagnostics.push(Diagnostic::error(
                    elem.span,
                    "`@asm` clobbers must be string literals",
                ));
            }
        }
    }

    fn check_asm_options(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::String(text) => self.check_asm_option_name(expr.span, text),
            ExprKind::ArrayLiteral {
                elems: ArrayElements::List(elems),
            } => {
                for elem in elems {
                    if let ExprKind::String(text) = &elem.kind {
                        self.check_asm_option_name(elem.span, text);
                    } else {
                        self.diagnostics.push(Diagnostic::error(
                            elem.span,
                            "`@asm` options must be string literals",
                        ));
                        self.check_expr(elem);
                    }
                }
            }
            ExprKind::ArrayLiteral { .. } => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "`@asm` options must be a list of string literals",
                ));
                self.check_expr(expr);
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "`@asm` field `options` must be a string literal or array literal",
                ));
                self.check_expr(expr);
            }
        }
    }

    fn check_asm_option_name(&mut self, span: Span, text: &str) {
        match decode_asm_string(text).as_deref() {
            Some("volatile") => {}
            Some(name) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown `@asm` option `{name}`"),
            )),
            None => self.diagnostics.push(Diagnostic::error(
                span,
                "invalid `@asm` option string literal",
            )),
        }
    }
}

fn decode_asm_string(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            '0' => out.push('\0'),
            _ => return None,
        }
    }
    Some(out)
}
