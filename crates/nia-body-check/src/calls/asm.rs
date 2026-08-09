// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::symbols::AsmConfigField;
use nia_ast::{ArrayElements, Expr, ExprKind, StringLiteral};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

impl<'a> BodyChecker<'a> {
    pub(super) fn check_asm_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        args: &[Expr],
    ) -> InternedTyId {
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                "builtin `asm` requires exactly one configuration argument",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.void();
        }
        let config = &args[0];
        let ExprKind::StructLiteral { fields } = &config.kind else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                config.span,
                "builtin `asm` expects an untyped struct literal configuration",
            ));
            self.check_expr(config);
            return self.void();
        };

        let mut has_code = false;
        for field in fields {
            match crate::symbols::asm_config_field(field.name) {
                Some(AsmConfigField::Code) => {
                    has_code = true;
                    if !matches!(field.value.kind, ExprKind::ByteString(_)) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            field.value.span,
                            "`asm` field `code` must be a byte string literal",
                        ));
                    }
                }
                Some(AsmConfigField::Inputs) => self.check_asm_inputs(&field.value),
                Some(AsmConfigField::Outputs) => self.check_asm_outputs(&field.value),
                Some(AsmConfigField::Clobbers) => self.check_asm_clobbers(&field.value),
                Some(AsmConfigField::Options) => self.check_asm_options(&field.value),
                None => {
                    let name = self.symbol_name(field.name);
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        field.span,
                        format!("unknown `asm` field `{name}`"),
                    ));
                    self.check_expr(&field.value);
                }
            }
        }
        if !has_code {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                builtin_span,
                "builtin `asm` requires a `code` byte string literal",
            ));
        }
        self.void()
    }

    fn check_asm_inputs(&mut self, expr: &Expr) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "`asm` field `inputs` must be a struct literal",
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
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "`asm` field `outputs` must be a struct literal",
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

    fn check_asm_operand_type(&mut self, span: Span, ty: InternedTyId, context: &str) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never))
            | Some(TyKind::Opaque) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("{context} cannot have an incomplete or uninhabited type"),
                ));
            }
            Some(
                TyKind::Tuple(_)
                | TyKind::Array { .. }
                | TyKind::Vector { .. }
                | TyKind::Slice { .. }
                | TyKind::SlicePointee { .. }
                | TyKind::TraitObject { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::Optional { .. }
                | TyKind::ErrorUnion { .. },
            ) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("{context} cannot use aggregate type directly"),
                ));
            }
            Some(TyKind::Range { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("{context} cannot use range type directly"),
                ));
            }
            Some(TyKind::Nominal { def_id, .. }) if !self.is_enum_def(*def_id) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("{context} cannot use aggregate type directly"),
                ));
            }
            Some(
                TyKind::Primitive(_)
                | TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::BuiltinTrait { .. }
                | TyKind::Nominal { .. },
            ) => {}
            Some(TyKind::BuiltinType(builtin)) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "{context} cannot use builtin type `{}` directly",
                        builtin.name()
                    ),
                ));
            }
            Some(
                TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::Projection { .. }
                | TyKind::ConstOnly
                | TyKind::Error,
            )
            | None => {}
        }
    }

    fn check_asm_clobbers(&mut self, expr: &Expr) {
        let ExprKind::ArrayLiteral {
            elems: ArrayElements::List(elems),
        } = &expr.kind
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "`asm` field `clobbers` must be an array literal of byte strings",
            ));
            self.check_expr(expr);
            return;
        };
        for elem in elems {
            if !matches!(elem.kind, ExprKind::ByteString(_)) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    elem.span,
                    "`asm` clobbers must be byte string literals",
                ));
            }
        }
    }

    fn check_asm_options(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::ByteString(text) => self.check_asm_option_name(expr.span, text),
            ExprKind::ArrayLiteral {
                elems: ArrayElements::List(elems),
            } => {
                for elem in elems {
                    if let ExprKind::ByteString(text) = &elem.kind {
                        self.check_asm_option_name(elem.span, text);
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            elem.span,
                            "`asm` options must be byte string literals",
                        ));
                        self.check_expr(elem);
                    }
                }
            }
            ExprKind::ArrayLiteral { .. } => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "`asm` options must be a list of byte string literals",
                ));
                self.check_expr(expr);
            }
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "`asm` field `options` must be a byte string literal or array literal",
                ));
                self.check_expr(expr);
            }
        }
    }

    fn check_asm_option_name(&mut self, span: Span, text: &StringLiteral) {
        match nia_literals::eval_byte_string_literal_parts(text.parts.iter().map(String::as_str))
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .as_deref()
        {
            Some("volatile") => {}
            Some(name) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("unknown `asm` option `{name}`"),
            )),
            None => self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "invalid `asm` option byte string literal",
            )),
        }
    }
}
