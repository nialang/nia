// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{
    decode_byte_char_literal, decode_byte_string_literal, decode_c_string_literal,
    decode_char_literal, decode_string_literal, numeric_literal_body, parse_int_literal,
};
use nia_ast::{ArrayElements, Expr, ExprKind, IndexArg};
use nia_body_ir::{BuiltinConst, BuiltinValue, PlaceBase, PlaceElem, StaticFieldInit, StaticInit};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_local_resolve::LocalUse;
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
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
            ExprKind::ByteChar(text) => decode_byte_char_literal(text)
                .map(StaticInit::Byte)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("invalid byte char literal `{text}` in static initializer"),
                    ));
                    StaticInit::Byte(0)
                }),
            ExprKind::Builtin { .. } => match self.builtin_values.get(&expr.span) {
                Some(BuiltinValue::Usize(value)) => StaticInit::Int(*value as i128),
                None => {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "builtin value is not representable as static data yet",
                    ));
                    StaticInit::Zero
                }
            },
            ExprKind::Ident(_) | ExprKind::Qualified { .. } => {
                if let Some(value) = self.static_comptime_int(expr) {
                    return StaticInit::Int(value);
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
                let ty = self
                    .expr_types
                    .get(&expr.span)
                    .copied()
                    .unwrap_or_else(|| self.error());
                StaticInit::Struct(
                    fields
                        .iter()
                        .map(|field| StaticFieldInit {
                            field: self
                                .field_def_for_struct_ty(ty, &field.name)
                                .unwrap_or_else(|| self.global_error_def()),
                            value: self.lower_static_init(&field.value),
                        })
                        .collect(),
                )
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
        let place = self.lower_static_place(expr);
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
        self.function_references
            .get(&expr.span)
            .map(|reference| (reference.def_id, reference.args.clone()))
    }

    fn lower_static_place(&mut self, expr: &Expr) -> nia_body_ir::TypedPlace {
        let ty = self
            .expr_types
            .get(&expr.span)
            .copied()
            .unwrap_or_else(|| self.error());
        let mut elems = Vec::new();
        let base = self.lower_static_place_inner(expr, &mut elems);
        nia_body_ir::TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_static_place_inner(&mut self, expr: &Expr, elems: &mut Vec<PlaceElem>) -> PlaceBase {
        if self.values.variant_enums.contains_key(&expr.span) {
            return PlaceBase::Local(LocalId(u32::MAX));
        }
        if let Some(def_id) = self.values.qualified_values.get(&expr.span).copied() {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::ModuleValue) => match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        PlaceBase::Global(self.global_def_id(*def_id))
                    }
                    _ => PlaceBase::Local(LocalId(u32::MAX)),
                },
                _ => PlaceBase::Local(LocalId(u32::MAX)),
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => PlaceBase::Deref(Box::new(self.lower_static_place_index_expr(expr))),
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_static_place_inner(lhs, elems);
                let lhs_ty = self
                    .expr_types
                    .get(&lhs.span)
                    .copied()
                    .unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .unwrap_or_else(|| self.global_error_def());
                elems.push(PlaceElem::Field(field));
                base
            }
            ExprKind::Index { lhs, index } => {
                let base = self.lower_static_place_inner(lhs, elems);
                if let IndexArg::Expr(index) = index {
                    elems.push(PlaceElem::Index(Box::new(
                        self.lower_static_place_index_expr(index),
                    )));
                }
                base
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr.span),
                    Some(nia_body_ir::BracketSuffixResolution::Index)
                ) {
                    let base = self.lower_static_place_inner(callee, elems);
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        elems.push(PlaceElem::Index(Box::new(
                            self.lower_static_place_index_expr(index),
                        )));
                    }
                    base
                } else {
                    PlaceBase::Local(LocalId(u32::MAX))
                }
            }
            _ => PlaceBase::Local(LocalId(u32::MAX)),
        }
    }

    fn lower_static_place_index_expr(&self, expr: &Expr) -> nia_body_ir::TypedExpr {
        let ty = self
            .expr_types
            .get(&expr.span)
            .copied()
            .unwrap_or_else(|| self.error());
        let kind = match &expr.kind {
            ExprKind::Integer(text) => {
                nia_body_ir::TypedExprKind::Integer(numeric_literal_body(text).to_string())
            }
            ExprKind::Builtin { .. } => self
                .builtin_values
                .get(&expr.span)
                .map(|value| match value {
                    BuiltinValue::Usize(value) => {
                        nia_body_ir::TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                })
                .unwrap_or(nia_body_ir::TypedExprKind::Error),
            ExprKind::Ident(_) | ExprKind::Qualified { .. } => self
                .static_comptime_int(expr)
                .map(|value| nia_body_ir::TypedExprKind::Integer(value.to_string()))
                .unwrap_or(nia_body_ir::TypedExprKind::Error),
            _ => nia_body_ir::TypedExprKind::Error,
        };
        nia_body_ir::TypedExpr {
            span: expr.span,
            ty,
            kind,
        }
    }

    fn static_comptime_int(&self, expr: &Expr) -> Option<i128> {
        if let Some(global_id) = self.global_comptime_use(expr.span) {
            return match self.global_comptime_value(global_id)? {
                nia_comptime_check::ComptimeValue::Int(value) => Some(value),
            };
        }
        if let Some(local_id) = self.local_comptime_use(expr.span) {
            return match self
                .comptime
                .values
                .get(&nia_comptime_check::ComptimeKey::Local(local_id))?
            {
                nia_comptime_check::ComptimeValue::Int(value) => Some(*value),
            };
        }
        None
    }
}
