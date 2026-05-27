// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind, IndexArg, SliceRange, UnaryOp};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::TyId;
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_ty::TyKind;
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_assignable(&mut self, expr: &Expr, context: &str) {
        if let Some(reason) = self.not_assignable_reason(expr) {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("{context} is not assignable: {reason}"),
            ));
        }
    }

    pub(crate) fn check_addressable(&mut self, expr: &Expr, context: &str) {
        if let Some(reason) = self.not_addressable_reason(expr) {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("{context} is not addressable: {reason}"),
            ));
        }
    }

    fn not_addressable_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.values.qualified_values.contains_key(&expr.span) {
            return None;
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::Local(_)) | Some(LocalUse::ModuleValue) => None,
                Some(LocalUse::ImportAlias) => Some("import alias is not a value place"),
                Some(LocalUse::TypePrefix) => Some("type prefix is not a value place"),
                Some(LocalUse::Unresolved) | None => Some("name is unresolved"),
            },
            ExprKind::Field { lhs, .. } => self.not_addressable_reason(lhs),
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Expr(_) => self.not_addressable_reason(lhs),
                IndexArg::Range(_) => Some("range index must be borrowed as a slice"),
            },
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(args) => {
                self.not_addressable_reason(callee)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => self.deref_addressable_reason(inner),
            _ => Some("expression is not a place"),
        }
    }

    fn deref_addressable_reason(&self, expr: &Expr) -> Option<&'static str> {
        match self
            .expr_types
            .get(&expr.span)
            .and_then(|ty| self.interner.get(*ty))
        {
            Some(TyKind::Pointer { .. } | TyKind::Error) => None,
            Some(_) => Some("expression does not dereference a pointer"),
            None => Some("pointer type is not known"),
        }
    }

    fn not_assignable_reason(&self, expr: &Expr) -> Option<&'static str> {
        if self.values.qualified_values.contains_key(&expr.span) {
            return Some("cross-module value is not assignable");
        }
        match &expr.kind {
            ExprKind::Ident(_) => self.ident_not_assignable_reason(expr.span),
            ExprKind::Field { lhs, .. } => self
                .not_assignable_reason(lhs)
                .or_else(|| self.auto_deref_not_assignable_reason(lhs)),
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Range(_) => Some("range index must be borrowed as a slice"),
                IndexArg::Expr(_) => self
                    .not_assignable_reason(lhs)
                    .or_else(|| self.auto_deref_not_assignable_reason(lhs))
                    .or_else(|| self.slice_not_assignable_reason(lhs)),
            },
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(args) => self
                .not_assignable_reason(callee)
                .or_else(|| self.auto_deref_not_assignable_reason(callee))
                .or_else(|| self.slice_not_assignable_reason(callee)),
            ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => self.deref_not_assignable_reason(inner),
            _ => Some("expression is not a place"),
        }
    }

    fn ident_not_assignable_reason(&self, span: Span) -> Option<&'static str> {
        match self.locals.uses.get(&span) {
            Some(LocalUse::Local(local_id)) => match self.locals.locals.get(*local_id) {
                Some(local) if local.kind == LocalKind::ConstBinding => Some("local is const"),
                Some(local) if local.kind == LocalKind::ComptimeBinding => {
                    Some("comptime binding has no storage")
                }
                Some(_) => None,
                None => Some("local definition is missing"),
            },
            Some(LocalUse::ModuleValue) => {
                if let Some(global_id) = self.values.qualified_values.get(&span).copied() {
                    return self.global_not_assignable_reason_global(global_id);
                }
                match self.values.names.get(&span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        self.global_not_assignable_reason(*def_id)
                    }
                    _ => Some("module value is unresolved"),
                }
            }
            Some(LocalUse::ImportAlias) => Some("import alias is not assignable"),
            Some(LocalUse::TypePrefix) => Some("type prefix is not a value place"),
            Some(LocalUse::Unresolved) | None => Some("name is unresolved"),
        }
    }

    fn global_not_assignable_reason(&self, def_id: DefId) -> Option<&'static str> {
        let Some(def) = self.defs.defs.get(def_id) else {
            return Some("global definition is missing");
        };
        match def.kind {
            DefKind::Global => match self.signatures.globals.get(&def_id) {
                Some(global) if global.is_const => Some("global is const"),
                Some(_) => None,
                None => Some("global signature is missing"),
            },
            DefKind::Comptime => Some("comptime binding has no storage"),
            DefKind::Function | DefKind::Method => Some("function item is not assignable"),
            _ => Some("definition is not a value place"),
        }
    }

    fn global_not_assignable_reason_global(
        &self,
        global_id: nia_ids::GlobalDefId,
    ) -> Option<&'static str> {
        if global_id.module_id == self.defs.module_id {
            return self.global_not_assignable_reason(global_id.def_id);
        }
        match self.program_globals.get(&global_id) {
            Some(global) if global.signature.is_const => Some("global is const"),
            Some(_) => None,
            None => Some("function item is not assignable"),
        }
    }

    fn deref_not_assignable_reason(&self, expr: &Expr) -> Option<&'static str> {
        match self
            .expr_types
            .get(&expr.span)
            .and_then(|ty| self.interner.get(*ty))
        {
            Some(TyKind::Pointer {
                is_const: false, ..
            }) => None,
            Some(TyKind::Pointer { is_const: true, .. }) => Some("pointer is const"),
            Some(TyKind::Error) => None,
            Some(_) => Some("expression does not dereference a pointer"),
            None => Some("pointer type is not known"),
        }
    }

    fn auto_deref_not_assignable_reason(&self, expr: &Expr) -> Option<&'static str> {
        match self
            .expr_types
            .get(&expr.span)
            .and_then(|ty| self.interner.get(*ty))
        {
            Some(TyKind::Pointer { is_const: true, .. }) => Some("pointer is const"),
            _ => None,
        }
    }

    fn slice_not_assignable_reason(&self, expr: &Expr) -> Option<&'static str> {
        match self
            .expr_types
            .get(&expr.span)
            .and_then(|ty| self.interner.get(*ty))
        {
            Some(TyKind::Slice { is_const: true, .. }) => Some("slice is const"),
            _ => None,
        }
    }

    pub(crate) fn check_slice_ref(
        &mut self,
        span: Span,
        lhs: &Expr,
        range: &SliceRange,
        is_const: bool,
        expected: Option<TyId>,
    ) -> TyId {
        let lhs_expected = self.array_expected_from_slice_expected(expected);
        let lhs_ty = self.check_expr_with_expected(lhs, lhs_expected);
        self.check_slice_range_bounds(range);
        if is_const {
            self.check_addressable(lhs, "slice target");
        } else {
            self.check_assignable(lhs, "slice target");
        }
        self.slice_result_type_with_context(span, lhs_ty, is_const)
    }

    pub(crate) fn check_slice_range_bounds(&mut self, range: &SliceRange) {
        if let Some(start) = &range.start {
            let start_ty = self.check_expr(start);
            self.expect_integer(start.span, start_ty, "slice range start");
        }
        if let Some(end) = &range.end {
            let end_ty = self.check_expr(end);
            self.expect_integer(end.span, end_ty, "slice range end");
        }
    }

    pub(crate) fn slice_result_type(&mut self, lhs_ty: TyId, is_const: bool) -> TyId {
        self.slice_result_type_with_context(Span::default(), lhs_ty, is_const)
    }

    fn slice_result_type_with_context(&mut self, span: Span, lhs_ty: TyId, is_const: bool) -> TyId {
        match self.interner.get(lhs_ty) {
            Some(TyKind::Array { elem, .. }) | Some(TyKind::Pointer { elem, .. }) => {
                self.interner.intern(TyKind::Slice {
                    is_const,
                    elem: *elem,
                })
            }
            Some(TyKind::Slice { elem, .. }) => self.interner.intern(TyKind::Slice {
                is_const,
                elem: *elem,
            }),
            Some(TyKind::Error) | None => self.error(),
            _ => {
                if span != Span::default() {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "slice range base must be an array, pointer, or slice",
                    ));
                }
                self.error()
            }
        }
    }

    pub(crate) fn index_result_type(&mut self, span: Span, lhs_ty: TyId) -> TyId {
        match self.interner.get(lhs_ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => *elem,
            Some(TyKind::Error) | None => self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "index base must be an array, pointer, or slice",
                ));
                self.error()
            }
        }
    }

    pub(crate) fn deref_result_type(&mut self, span: Span, ty: TyId) -> TyId {
        match self.interner.get(ty) {
            Some(TyKind::Pointer { elem, .. })
                if self.normalization.normalize(*elem) == self.void() =>
            {
                self.diagnostics
                    .push(Diagnostic::error(span, "cannot dereference `&void`"));
                self.error()
            }
            Some(TyKind::Pointer { elem, .. }) => *elem,
            Some(TyKind::Error) | None => self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "cannot dereference non-pointer type",
                ));
                self.error()
            }
        }
    }

    pub(crate) fn is_place_expr(&self, expr: &Expr) -> bool {
        if self.values.qualified_values.contains_key(&expr.span) {
            return true;
        }
        match &expr.kind {
            ExprKind::Ident(_) => matches!(
                self.locals.uses.get(&expr.span),
                Some(LocalUse::Local(_) | LocalUse::ModuleValue)
            ),
            ExprKind::Field { lhs, .. } => self.is_place_expr(lhs),
            ExprKind::Index {
                lhs,
                index: IndexArg::Expr(_),
            } => self.is_place_expr(lhs),
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(args) => {
                self.is_place_expr(callee)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref, ..
            } => true,
            _ => false,
        }
    }

    fn bracket_suffix_is_index(&self, args: &[BracketArg]) -> bool {
        args.len() == 1 && args.first().is_some_and(|arg| arg.expr.is_some())
    }
}
