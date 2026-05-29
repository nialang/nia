// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use crate::literals::{
    decode_byte_char, decode_byte_string_literal, decode_c_string_literal, decode_char_literal,
    decode_string_literal, numeric_literal_body, parse_int_literal,
};
use nia_ast::{ArrayElements, Expr, ExprKind};
use nia_backend_ir::{StaticFieldInit, StaticInit};
use nia_body_ir::{BuiltinValue, PlaceBase};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_static_init(&mut self, expr: &Expr) -> StaticInit {
        match &expr.kind {
            ExprKind::Integer(text) => {
                parse_int_literal(text)
                    .map(StaticInit::Int)
                    .unwrap_or_else(|| {
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            format!("invalid integer literal `{text}` in static initializer"),
                        ));
                        StaticInit::Zero
                    })
            }
            ExprKind::Float(text) => StaticInit::Float(numeric_literal_body(text).to_string()),
            ExprKind::Bool(value) => StaticInit::Bool(*value),
            ExprKind::String(literal) => decode_string_literal(literal)
                .map(StaticInit::Chars)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "invalid string literal in static initializer",
                    ));
                    StaticInit::Chars(Vec::new())
                }),
            ExprKind::ByteString(literal) => decode_byte_string_literal(literal)
                .map(StaticInit::Bytes)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "invalid byte string literal in static initializer",
                    ));
                    StaticInit::Bytes(Vec::new())
                }),
            ExprKind::CString(literal) => decode_c_string_literal(literal)
                .map(StaticInit::Bytes)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "invalid C string literal in static initializer",
                    ));
                    StaticInit::Bytes(Vec::new())
                }),
            ExprKind::Char(text) => decode_char_literal(text)
                .map(StaticInit::Char)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("invalid char literal `{text}` in static initializer"),
                    ));
                    StaticInit::Char(0)
                }),
            ExprKind::ByteChar(text) => decode_byte_char(text)
                .map(StaticInit::Byte)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("invalid byte char literal `{text}` in static initializer"),
                    ));
                    StaticInit::Byte(0)
                }),
            ExprKind::Builtin { .. } => {
                match self.input.body_check.ir.builtin_values.get(&expr.span) {
                    Some(BuiltinValue::Usize(value)) => StaticInit::Int(*value as i128),
                    None => {
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            "builtin value is not representable as static data yet",
                        ));
                        StaticInit::Zero
                    }
                }
            }
            ExprKind::Ident(_) => {
                if let Some(def_id) = self.comptime_global_id_for_expr(expr)
                    && let Some(binding) = self.comptime_binding_for(def_id)
                    && let Some(value) = &binding.value
                {
                    if self.comptime_global_stack.contains(&def_id) {
                        return StaticInit::Zero;
                    }
                    self.comptime_global_stack.push(def_id);
                    let init = self.lower_static_init(value);
                    self.comptime_global_stack.pop();
                    return init;
                }
                if let Some(value) = self.local_comptime_value(expr).cloned() {
                    return self.lower_static_init(&value);
                }
                StaticInit::Zero
            }
            ExprKind::ArrayLiteral { elems } => match elems {
                ArrayElements::List(elems) => StaticInit::Array(
                    elems
                        .iter()
                        .map(|elem| self.lower_static_init(elem))
                        .collect(),
                ),
                ArrayElements::Repeat { value, count } => StaticInit::Repeat {
                    value: Box::new(self.lower_static_init(value)),
                    count: self.lower_array_repeat_count(count),
                },
            },
            ExprKind::StructLiteral { fields } => {
                let ty = self.expr_ty(expr).unwrap_or_else(|| self.error_ty());
                let field_inits = fields
                    .iter()
                    .map(|field| StaticFieldInit {
                        field: self
                            .field_def_for_struct_ty(ty, &field.name)
                            .unwrap_or_else(|| self.global_error_def()),
                        value: self.lower_static_init(&field.value),
                    })
                    .collect::<Vec<_>>();
                let is_union = self.nominal_global_def(ty).is_some_and(|def_id| {
                    self.input.signatures.unions.contains_key(&def_id.def_id)
                });
                let _ = is_union;
                StaticInit::Struct(field_inits)
            }
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Neg,
                ..
            }
            | ExprKind::Binary { .. } => {
                let mut env = nia_comptime_engine::EmptyEnv;
                nia_comptime_engine::eval_int_expr(expr, &mut env)
                    .map(StaticInit::Int)
                    .unwrap_or_else(|err| {
                        self.diagnostics.push(Diagnostic::error(
                            expr.span,
                            format!(
                                "invalid integer constant in static initializer: {}",
                                err.message
                            ),
                        ));
                        StaticInit::Zero
                    })
            }
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Ref | nia_ast::UnaryOp::RefConst,
                expr,
            } => self.lower_static_address_init(expr),
            ExprKind::Cast { expr, .. } => self.lower_static_init(expr),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "global initializer is not representable as static data yet",
                ));
                StaticInit::Zero
            }
        }
    }

    fn lower_static_address_init(&mut self, expr: &Expr) -> StaticInit {
        if let Some((function, args)) = self.static_function_address(expr) {
            return StaticInit::AddrOfFunction { function, args };
        }
        let place = self.lower_place(expr);
        match place.base {
            PlaceBase::Global(global) => StaticInit::AddrOfGlobal {
                global,
                path: place.elems,
            },
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    expr.span,
                    "global address initializer must refer to global storage",
                ));
                StaticInit::Zero
            }
        }
    }

    fn static_function_address(&self, expr: &Expr) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        self.input
            .body_check
            .ir
            .function_references
            .get(&expr.span)
            .map(|reference| (reference.def_id, reference.args.clone()))
    }
}
