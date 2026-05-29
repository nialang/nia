// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_ast::{Expr, ExprKind, IndexArg};
use nia_body_ir::{BuiltinConst, PlaceBase, PlaceElem, TypedExpr, TypedExprKind, TypedPlace};
use nia_ids::LocalId;
use nia_local_resolve::LocalUse;
use nia_value_resolve::ValueNameResolution;

use crate::literals::numeric_literal_body;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_array_repeat_count(&mut self, count: &Expr) -> u64 {
        nia_comptime_engine::eval_array_len_expr(count, self).unwrap_or(0)
    }

    pub(crate) fn local_comptime_value(
        &self,
        expr: &Expr,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&expr.span) else {
            return None;
        };
        self.input
            .comptime
            .values
            .get(&nia_comptime_check::ComptimeKey::Local(*local_id))
            .cloned()
    }

    pub(crate) fn local_comptime_id_for_span(&self, span: nia_span::Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        self.input
            .comptime
            .values
            .contains_key(&nia_comptime_check::ComptimeKey::Local(*local_id))
            .then_some(*local_id)
    }

    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error_ty());
        let mut elems = Vec::new();
        let base = self.lower_place_inner(expr, &mut elems);
        TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_place_inner(&mut self, expr: &Expr, elems: &mut Vec<PlaceElem>) -> PlaceBase {
        if self.input.values.variant_enums.contains_key(&expr.span) {
            return PlaceBase::Local(LocalId(u32::MAX));
        }
        if let Some(def_id) = self.input.values.qualified_values.get(&expr.span).copied() {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.input.locals.uses.get(&expr.span) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(*local),
                Some(LocalUse::ModuleValue) => match self.input.values.names.get(&expr.span) {
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
                let base = self.lower_place_inner(lhs, elems);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error_ty());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .unwrap_or_else(|| self.global_error_def());
                elems.push(PlaceElem::Field(field));
                base
            }
            ExprKind::Index { lhs, index } => {
                let base = self.lower_place_inner(lhs, elems);
                if let IndexArg::Expr(index) = index {
                    elems.push(PlaceElem::Index(Box::new(
                        self.lower_static_place_index_expr(index),
                    )));
                }
                base
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.input
                        .body_check
                        .ir
                        .bracket_suffix_resolutions
                        .get(&expr.span),
                    Some(nia_body_ir::BracketSuffixResolution::Index)
                ) {
                    let base = self.lower_place_inner(callee, elems);
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

    fn lower_static_place_index_expr(&self, expr: &Expr) -> TypedExpr {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error_ty());
        let kind = match &expr.kind {
            ExprKind::Integer(text) => {
                TypedExprKind::Integer(numeric_literal_body(text).to_string())
            }
            ExprKind::Builtin { .. } => self
                .input
                .body_check
                .ir
                .builtin_values
                .get(&expr.span)
                .map(|value| match value {
                    nia_body_ir::BuiltinValue::Usize(value) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                })
                .unwrap_or(TypedExprKind::Error),
            ExprKind::Ident(_) | ExprKind::Qualified { .. } => self
                .static_comptime_int(expr)
                .map(|value| TypedExprKind::Integer(value.to_string()))
                .unwrap_or(TypedExprKind::Error),
            _ => TypedExprKind::Error,
        };
        TypedExpr {
            span: expr.span,
            ty,
            kind,
        }
    }

    fn static_comptime_int(&self, expr: &Expr) -> Option<i128> {
        let value = if let Some(global_id) = self.comptime_global_id_for_expr(expr) {
            self.input
                .comptime
                .values
                .get(&nia_comptime_check::ComptimeKey::Global(global_id))
        } else if let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&expr.span) {
            self.input
                .comptime
                .values
                .get(&nia_comptime_check::ComptimeKey::Local(*local_id))
        } else {
            None
        }?;
        match value {
            nia_comptime_check::ComptimeValue::Int(value) => Some(*value),
        }
    }
}
