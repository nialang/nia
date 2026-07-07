// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::literals::decode_byte_string_literal;
use crate::symbols::AsmConfigField;
use nia_body_ir::{AsmOption, TypedAsmInput, TypedAsmOutput, TypedInlineAsm};
use nia_symbol::known;

impl<'a> BodyChecker<'a> {
    pub(super) fn lower_inline_asm(&mut self, config: &Expr) -> TypedExprKind {
        let ExprKind::StructLiteral { fields } = &config.kind else {
            return TypedExprKind::Error;
        };
        let mut code = String::new();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut clobbers = Vec::new();
        let mut options = Vec::new();
        for field in fields {
            match crate::symbols::asm_config_field(field.name) {
                Some(AsmConfigField::Code) => {
                    if let ExprKind::ByteString(literal) = &field.value.kind {
                        code = String::from_utf8(
                            decode_byte_string_literal(literal).unwrap_or_default(),
                        )
                        .unwrap_or_default();
                    }
                }
                Some(AsmConfigField::Inputs) => self.lower_asm_inputs(&field.value, &mut inputs),
                Some(AsmConfigField::Outputs) => self.lower_asm_outputs(&field.value, &mut outputs),
                Some(AsmConfigField::Clobbers) => {
                    self.lower_asm_clobbers(&field.value, &mut clobbers)
                }
                Some(AsmConfigField::Options) => self.lower_asm_options(&field.value, &mut options),
                None => {}
            }
        }
        TypedExprKind::InlineAsm(TypedInlineAsm {
            code,
            inputs,
            outputs,
            clobbers,
            options,
        })
    }

    fn lower_asm_inputs(&mut self, expr: &Expr, out: &mut Vec<TypedAsmInput>) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            return;
        };
        for field in fields {
            out.push(TypedAsmInput {
                constraint: self.asm_input_constraint(field.name),
                value: self.lower_expr(&field.value),
                span: field.span,
            });
        }
    }

    fn lower_asm_outputs(&mut self, expr: &Expr, out: &mut Vec<TypedAsmOutput>) {
        let ExprKind::StructLiteral { fields } = &expr.kind else {
            return;
        };
        for field in fields {
            out.push(TypedAsmOutput {
                constraint: self.asm_output_constraint(field.name),
                place: self.lower_place(&field.value),
                span: field.span,
            });
        }
    }

    fn lower_asm_clobbers(&mut self, expr: &Expr, out: &mut Vec<String>) {
        let ExprKind::ArrayLiteral {
            elems: ArrayElements::List(elems),
        } = &expr.kind
        else {
            return;
        };
        for elem in elems {
            if let ExprKind::ByteString(literal) = &elem.kind
                && let Ok(clobber) =
                    String::from_utf8(decode_byte_string_literal(literal).unwrap_or_default())
            {
                out.push(clobber);
            }
        }
    }

    fn lower_asm_options(&mut self, expr: &Expr, out: &mut Vec<AsmOption>) {
        match &expr.kind {
            ExprKind::ByteString(literal) => {
                if let Some(option) = asm_option_from_string(literal) {
                    out.push(option);
                }
            }
            ExprKind::ArrayLiteral {
                elems: ArrayElements::List(elems),
            } => {
                for elem in elems {
                    if let ExprKind::ByteString(literal) = &elem.kind
                        && let Some(option) = asm_option_from_string(literal)
                    {
                        out.push(option);
                    }
                }
            }
            _ => {}
        }
    }
    fn asm_input_constraint(&self, name: nia_symbol::SymbolId) -> String {
        match name {
            known::REG => "r".to_string(),
            known::FREG => "f".to_string(),
            _ => format!("{{{}}}", self.symbol_name(name)),
        }
    }

    fn asm_output_constraint(&self, name: nia_symbol::SymbolId) -> String {
        match name {
            known::REG => "=r".to_string(),
            known::FREG => "=f".to_string(),
            _ => format!("={{{}}}", self.symbol_name(name)),
        }
    }
}

fn asm_option(name: &str) -> Option<AsmOption> {
    match name {
        "volatile" => Some(AsmOption::Volatile),
        _ => None,
    }
}

fn asm_option_from_string(literal: &nia_ast::StringLiteral) -> Option<AsmOption> {
    let name = String::from_utf8(decode_byte_string_literal(literal)?).ok()?;
    asm_option(&name)
}
