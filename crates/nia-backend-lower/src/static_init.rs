// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use crate::literals::{
    decode_byte_char, decode_string_literal, numeric_literal_body, parse_int_literal,
};
use nia_ast::{ArrayElements, Expr, ExprKind};
use nia_backend_ir::{PlaceBase, StaticFieldInit, StaticInit};
use nia_body_check::BuiltinValue;
use nia_defs::DefKind;
use nia_diagnostic::Diagnostic;
use nia_value_resolve::ValueNameResolution;

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
            ExprKind::String(text) => decode_string_literal(text)
                .map(StaticInit::Bytes)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("invalid string literal `{text}` in static initializer"),
                    ));
                    StaticInit::Bytes(Vec::new())
                }),
            ExprKind::Char(text) => StaticInit::Char(text.clone()),
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
                match self.input.body_check.builtin_values.get(&expr.span) {
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
                    count: nia_comptime_engine::eval_array_len_text(&count.text).unwrap_or_else(
                        |err| {
                            self.diagnostics.push(Diagnostic::error(
                                count.span,
                                format!("invalid repeat count: {err}"),
                            ));
                            0
                        },
                    ),
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
        if let Some(function) = self.static_function_address(expr) {
            return StaticInit::AddrOfFunction(function);
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

    fn static_function_address(&self, expr: &Expr) -> Option<nia_ids::GlobalDefId> {
        let ExprKind::Ident(_) = &expr.kind else {
            return None;
        };
        if let Some(global_id) = self.input.values.qualified_values.get(&expr.span).copied() {
            let kind = self
                .input
                .all_defs
                .iter()
                .find(|defs| defs.module_id == global_id.module_id)
                .and_then(|defs| defs.defs.get(global_id.def_id))
                .map(|def| def.kind);
            return match kind {
                Some(DefKind::Function | DefKind::Method) => Some(global_id),
                _ => None,
            };
        }
        let ValueNameResolution::Def(def_id) = self.input.values.names.get(&expr.span)? else {
            return None;
        };
        match self.input.defs.defs.get(*def_id).map(|def| def.kind) {
            Some(DefKind::Function | DefKind::Method) => Some(self.global_def_id(*def_id)),
            _ => None,
        }
    }
}
