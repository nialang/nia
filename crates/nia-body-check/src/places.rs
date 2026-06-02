// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{Expr, ExprKind, IndexArg, SliceRange, UnaryOp};
use nia_body_ir::BracketSuffixResolution;
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_ty::{BuiltinTrait, PrimitiveTy, RangeTyKind, TraitId, TyKind};
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_assignment_lhs(&mut self, expr: &Expr) -> InternedTyId {
        let ty = match &expr.kind {
            ExprKind::Field { lhs, name } => {
                let lhs_ty = self.check_assignment_lhs(lhs);
                self.field_access_type_from_lhs_ty(expr.span, lhs_ty, name)
            }
            ExprKind::Index {
                lhs,
                index: IndexArg::Expr(index),
            } => {
                let lhs_ty = self.check_assignment_lhs(lhs);
                let index_ty = self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::Index, index);
                self.expect_integer(index.span, index_ty, "index");
                let index_ty = self
                    .expr_types
                    .get(&index.span)
                    .copied()
                    .unwrap_or(index_ty);
                if index_ty == self.error() {
                    return self.error();
                }
                self.index_result_type_for_write_index(expr.span, lhs_ty, index_ty)
            }
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(expr.span) => {
                let lhs_ty = self.check_assignment_lhs(callee);
                if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                    let index_ty =
                        self.check_index_expr_for_trait(lhs_ty, BuiltinTrait::Index, index);
                    self.expect_integer(index.span, index_ty, "index");
                    let index_ty = self
                        .expr_types
                        .get(&index.span)
                        .copied()
                        .unwrap_or(index_ty);
                    if index_ty == self.error() {
                        return self.error();
                    }
                    self.index_result_type_for_write_index(expr.span, lhs_ty, index_ty)
                } else {
                    self.check_expr(expr)
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => {
                let inner_ty = self.check_expr(inner);
                self.deref_result_type_for_write(expr.span, inner_ty)
            }
            _ => self.check_expr(expr),
        };
        self.record_expr_type(expr.span, ty);
        ty
    }

    pub(crate) fn assignable_expr_type(&mut self, expr: &Expr) -> InternedTyId {
        let ty = self
            .expr_types
            .get(&expr.span)
            .copied()
            .unwrap_or_else(|| self.error());
        match &expr.kind {
            ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => {
                let Some(inner_ty) = self.expr_types.get(&inner.span).copied() else {
                    return ty;
                };
                let target = self.interner.intern(TyKind::Projection {
                    self_ty: inner_ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::Deref),
                    trait_args: Vec::new(),
                    name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(target)
            }
            ExprKind::Index {
                lhs,
                index: IndexArg::Expr(index),
            } => {
                let Some(lhs_ty) = self.expr_types.get(&lhs.span).copied() else {
                    return ty;
                };
                let Some(index_ty) = self.expr_types.get(&index.span).copied() else {
                    return ty;
                };
                let output = self.interner.intern(TyKind::Projection {
                    self_ty: lhs_ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::Index),
                    trait_args: vec![index_ty],
                    name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(output)
            }
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(expr.span) => {
                let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) else {
                    return ty;
                };
                let Some(lhs_ty) = self.expr_types.get(&callee.span).copied() else {
                    return ty;
                };
                let Some(index_ty) = self.expr_types.get(&index.span).copied() else {
                    return ty;
                };
                let output = self.interner.intern(TyKind::Projection {
                    self_ty: lhs_ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::Index),
                    trait_args: vec![index_ty],
                    name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(output)
            }
            _ => ty,
        }
    }

    pub(crate) fn check_assignable(&mut self, expr: &Expr, context: &str) {
        if let Some(reason) = self.not_assignable_reason(expr) {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("{context} is not assignable: {reason}"),
            ));
        }
    }

    pub(crate) fn check_reference_target(&mut self, expr: &Expr, context: &str, is_const: bool) {
        let ty = self.expr_types.get(&expr.span).copied();
        self.check_reference_target_with_ty(expr, context, is_const, ty);
    }

    pub(crate) fn check_reference_target_with_ty(
        &mut self,
        expr: &Expr,
        context: &str,
        is_const: bool,
        ty: Option<InternedTyId>,
    ) {
        let reason = if self.is_place_expr(expr) {
            if is_const {
                self.not_addressable_reason(expr)
            } else {
                self.not_assignable_reason(expr)
            }
        } else {
            self.not_materializable_reason(ty)
        };
        if let Some(reason) = reason {
            let property = if is_const {
                "addressable"
            } else {
                "assignable"
            };
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!("{context} is not {property}: {reason}"),
            ));
        }
    }

    fn not_materializable_reason(&self, ty: Option<InternedTyId>) -> Option<&'static str> {
        let Some(ty) = ty else {
            return Some("expression type is not known");
        };
        if self.is_invalid_temporary_type(ty) {
            Some("temporary cannot have void or never type")
        } else {
            None
        }
    }

    fn not_addressable_reason(&mut self, expr: &Expr) -> Option<&'static str> {
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
            ExprKind::BracketSuffix { callee, .. } if self.bracket_suffix_is_index(expr.span) => {
                self.not_addressable_reason(callee)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => self.deref_addressable_reason(inner),
            _ => Some("expression is not a place"),
        }
    }

    fn deref_addressable_reason(&mut self, expr: &Expr) -> Option<&'static str> {
        let Some(ty) = self.expr_types.get(&expr.span).copied() else {
            return Some("pointer type is not known");
        };
        let has_deref_const = self.current_context_proves_trait_obligation(
            ty,
            TraitId::Builtin(BuiltinTrait::DerefConst),
            Vec::new(),
        );
        match self.interner.get(ty) {
            Some(TyKind::Error) => None,
            Some(_) if has_deref_const => None,
            Some(_) => Some("expression does not implement DerefConst"),
            None => Some("pointer type is not known"),
        }
    }

    fn not_assignable_reason(&mut self, expr: &Expr) -> Option<&'static str> {
        if self.values.qualified_values.contains_key(&expr.span) {
            return Some("cross-module value is not assignable");
        }
        match &expr.kind {
            ExprKind::Ident(_) => self.ident_not_assignable_reason(expr.span),
            ExprKind::Field { lhs, .. } => self.not_assignable_reason(lhs),
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Range(_) => Some("range index must be borrowed as a slice"),
                IndexArg::Expr(index) => self
                    .not_assignable_reason(lhs)
                    .or_else(|| self.index_write_not_assignable_reason(lhs, index)),
            },
            ExprKind::BracketSuffix { callee, args } if self.bracket_suffix_is_index(expr.span) => {
                self.not_assignable_reason(callee).or_else(|| {
                    args.first()
                        .and_then(|arg| arg.expr.as_ref())
                        .map_or(Some("index type is not known"), |index| {
                            self.index_write_not_assignable_reason(callee, index)
                        })
                })
            }
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

    fn deref_not_assignable_reason(&mut self, expr: &Expr) -> Option<&'static str> {
        let Some(ty) = self.expr_types.get(&expr.span).copied() else {
            return Some("pointer type is not known");
        };
        let has_deref = self.current_context_proves_trait_obligation(
            ty,
            TraitId::Builtin(BuiltinTrait::Deref),
            Vec::new(),
        );
        match self.interner.get(ty) {
            Some(TyKind::Error) => None,
            Some(_) if has_deref => None,
            Some(TyKind::Pointer { is_const: true, .. }) => Some("pointer is const"),
            Some(_) => Some("expression does not implement Deref"),
            None => Some("pointer type is not known"),
        }
    }

    pub(crate) fn check_slice_ref(
        &mut self,
        span: Span,
        lhs: &Expr,
        range: &SliceRange,
        is_const: bool,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let lhs_expected = self.array_expected_from_slice_expected(expected);
        let lhs_ty = self.check_expr_with_expected(lhs, lhs_expected);
        let range_ty = self.check_slice_range_bounds(range);
        if is_const {
            self.check_reference_target(lhs, "slice target", true);
        } else {
            self.check_reference_target(lhs, "slice target", false);
        }
        self.slice_result_type_with_context(span, lhs_ty, is_const, range_ty)
    }

    pub(crate) fn check_slice_range_bounds(&mut self, range: &SliceRange) -> InternedTyId {
        let expected = self.slice_range_expected_ty(range);
        let usize_ty = self.primitive(PrimitiveTy::Usize);
        if let Some(start) = &range.start {
            let start_ty = self.check_expr_with_expected(start, Some(usize_ty));
            self.expect_expr_type(start, usize_ty, start_ty, "slice range start");
            self.expect_integer(start.span, start_ty, "slice range start");
        }
        if let Some(end) = &range.end {
            let end_ty = self.check_expr_with_expected(end, Some(usize_ty));
            self.expect_expr_type(end, usize_ty, end_ty, "slice range end");
            self.expect_integer(end.span, end_ty, "slice range end");
        }
        expected
    }

    fn slice_range_expected_ty(&mut self, range: &SliceRange) -> InternedTyId {
        let kind = match (range.start.is_some(), range.end.is_some(), range.inclusive) {
            (true, true, false) => RangeTyKind::Exclusive,
            (true, true, true) => RangeTyKind::Inclusive,
            (true, false, false) => RangeTyKind::From,
            (false, true, false) => RangeTyKind::To,
            (false, true, true) => RangeTyKind::ToInclusive,
            (false, false, false) => RangeTyKind::Full,
            (true, false, true) | (false, false, true) => RangeTyKind::Full,
        };
        let bound = (kind != RangeTyKind::Full).then(|| self.primitive(PrimitiveTy::Usize));
        self.interner.intern(TyKind::Range { kind, bound })
    }

    pub(crate) fn slice_result_type(
        &mut self,
        lhs_ty: InternedTyId,
        is_const: bool,
    ) -> InternedTyId {
        // Used after the caller has already emitted the source-level error for
        // a non-borrowed range index. Pass a default span so trait probing does
        // not add a second, less helpful diagnostic.
        let range_ty = self.interner.intern(TyKind::Range {
            kind: RangeTyKind::Full,
            bound: None,
        });
        self.slice_result_type_with_context(Span::default(), lhs_ty, is_const, range_ty)
    }

    fn slice_result_type_with_context(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        is_const: bool,
        range_ty: InternedTyId,
    ) -> InternedTyId {
        if matches!(self.interner.get(lhs_ty), Some(TyKind::Error) | None) {
            return self.error();
        }
        if is_const {
            if self.current_context_proves_trait_obligation(
                lhs_ty,
                TraitId::Builtin(BuiltinTrait::SliceConst),
                vec![range_ty],
            ) {
                let output = self.interner.intern(TyKind::Projection {
                    self_ty: lhs_ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::SliceConst),
                    trait_args: vec![range_ty],
                    name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
                });
                return self.normalize_projection(output);
            }
            if span != Span::default() {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait bound not satisfied: {}: {}",
                        self.ty_name(lhs_ty),
                        self.builtin_trait_ty_name(BuiltinTrait::SliceConst, &[range_ty])
                    ),
                ));
            }
            return self.error();
        }
        if self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(BuiltinTrait::Slice),
            vec![range_ty],
        ) {
            let output = self.interner.intern(TyKind::Projection {
                self_ty: lhs_ty,
                trait_id: TraitId::Builtin(BuiltinTrait::Slice),
                trait_args: vec![range_ty],
                name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
            });
            return self.normalize_projection(output);
        }
        if span != Span::default() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(BuiltinTrait::Slice, &[range_ty])
                ),
            ));
        }
        self.error()
    }

    pub(crate) fn index_result_type_for_index(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        index_ty: InternedTyId,
    ) -> InternedTyId {
        if matches!(self.interner.get(lhs_ty), Some(TyKind::Error) | None) {
            return self.error();
        }
        let trait_args = vec![index_ty];
        if !self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(BuiltinTrait::IndexConst),
            trait_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(BuiltinTrait::IndexConst, &trait_args)
                ),
            ));
            return self.error();
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: lhs_ty,
            trait_id: TraitId::Builtin(BuiltinTrait::IndexConst),
            trait_args,
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(output)
    }

    pub(crate) fn index_result_type_for_write_index(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        index_ty: InternedTyId,
    ) -> InternedTyId {
        if matches!(self.interner.get(lhs_ty), Some(TyKind::Error) | None) {
            return self.error();
        }
        let trait_args = vec![index_ty];
        if !self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(BuiltinTrait::Index),
            trait_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(lhs_ty),
                    self.builtin_trait_ty_name(BuiltinTrait::Index, &trait_args)
                ),
            ));
            return self.error();
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: lhs_ty,
            trait_id: TraitId::Builtin(BuiltinTrait::Index),
            trait_args,
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(output)
    }

    pub(crate) fn check_index_expr_for_trait(
        &mut self,
        lhs_ty: InternedTyId,
        trait_id: BuiltinTrait,
        index: &Expr,
    ) -> InternedTyId {
        match self.index_literal_expected_type(lhs_ty, trait_id, index) {
            IndexLiteralExpectedType::Known(index_expected) => {
                let index_ty = self.check_expr_with_expected(index, Some(index_expected));
                self.expect_expr_type(index, index_expected, index_ty, "index");
                index_ty
            }
            IndexLiteralExpectedType::Ambiguous => {
                self.check_expr(index);
                self.diagnostics.push(Diagnostic::error(
                    index.span,
                    format!(
                        "ambiguous index literal type for {}; add a literal suffix or type annotation",
                        self.builtin_trait_ty_name(trait_id, &[])
                    ),
                ));
                self.error()
            }
            IndexLiteralExpectedType::Unknown => self.check_expr(index),
        }
    }

    fn index_literal_expected_type(
        &mut self,
        lhs_ty: InternedTyId,
        trait_id: BuiltinTrait,
        index: &Expr,
    ) -> IndexLiteralExpectedType {
        if self.numeric_literal_has_suffix(index) || !self.is_numeric_literal_expr(index) {
            return IndexLiteralExpectedType::Unknown;
        }
        let candidates = self.visible_trait_arg_candidates(lhs_ty, TraitId::Builtin(trait_id));
        let mut index_candidates = Vec::new();
        for candidate in candidates {
            let [index_ty] = candidate.as_slice() else {
                continue;
            };
            let index_ty = self.normalization.normalize(*index_ty);
            if self.is_integer(index_ty)
                && !index_candidates.iter().any(|existing| {
                    self.types_equivalent_without_projection_resolution(*existing, index_ty)
                })
            {
                index_candidates.push(index_ty);
            }
        }
        match index_candidates.as_slice() {
            [index_ty] => IndexLiteralExpectedType::Known(*index_ty),
            [] => IndexLiteralExpectedType::Unknown,
            _ => IndexLiteralExpectedType::Ambiguous,
        }
    }

    fn index_write_not_assignable_reason(
        &mut self,
        lhs: &Expr,
        index: &Expr,
    ) -> Option<&'static str> {
        let Some(lhs_ty) = self.expr_types.get(&lhs.span).copied() else {
            return Some("index base type is not known");
        };
        let Some(index_ty) = self.expr_types.get(&index.span).copied() else {
            return Some("index type is not known");
        };
        let has_index = self.current_context_proves_trait_obligation(
            lhs_ty,
            TraitId::Builtin(BuiltinTrait::Index),
            vec![index_ty],
        );
        match self.interner.get(self.normalization.normalize(lhs_ty)) {
            Some(TyKind::Error) => None,
            Some(_) if has_index => None,
            Some(TyKind::Pointer { is_const: true, .. }) => Some("pointer is const"),
            Some(TyKind::Slice { is_const: true, .. }) => Some("slice is const"),
            _ => Some("expression does not implement Index"),
        }
    }

    pub(crate) fn deref_result_type(&mut self, span: Span, ty: InternedTyId) -> InternedTyId {
        let has_deref_const = self.current_context_proves_trait_obligation(
            ty,
            TraitId::Builtin(BuiltinTrait::DerefConst),
            Vec::new(),
        );
        match self.interner.get(ty) {
            Some(TyKind::Pointer { elem, .. })
                if self.normalization.normalize(*elem) == self.void() =>
            {
                self.diagnostics
                    .push(Diagnostic::error(span, "cannot dereference `&void`"));
                self.error()
            }
            Some(TyKind::Error) | None => self.error(),
            _ if has_deref_const => {
                let target = self.interner.intern(TyKind::Projection {
                    self_ty: ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::DerefConst),
                    trait_args: Vec::new(),
                    name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(target)
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait bound not satisfied: {}: {}",
                        self.ty_name(ty),
                        self.builtin_trait_ty_name(BuiltinTrait::DerefConst, &[])
                    ),
                ));
                self.error()
            }
        }
    }

    pub(crate) fn deref_result_type_for_write(
        &mut self,
        span: Span,
        ty: InternedTyId,
    ) -> InternedTyId {
        let has_deref = self.current_context_proves_trait_obligation(
            ty,
            TraitId::Builtin(BuiltinTrait::Deref),
            Vec::new(),
        );
        match self.interner.get(ty) {
            Some(TyKind::Pointer { elem, .. })
                if self.normalization.normalize(*elem) == self.void() =>
            {
                self.diagnostics
                    .push(Diagnostic::error(span, "cannot dereference `&void`"));
                self.error()
            }
            Some(TyKind::Error) | None => self.error(),
            _ if has_deref => {
                let target = self.interner.intern(TyKind::Projection {
                    self_ty: ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::Deref),
                    trait_args: Vec::new(),
                    name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(target)
            }
            _ => self.deref_result_type(span, ty),
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
            ExprKind::BracketSuffix { callee, .. } if self.bracket_suffix_is_index(expr.span) => {
                self.is_place_expr(callee)
            }
            ExprKind::Unary {
                op: UnaryOp::Deref, ..
            } => true,
            _ => false,
        }
    }

    fn bracket_suffix_is_index(&self, span: Span) -> bool {
        matches!(
            self.bracket_suffix_resolutions.get(&span),
            Some(BracketSuffixResolution::Index)
        )
    }
}

enum IndexLiteralExpectedType {
    Known(InternedTyId),
    Ambiguous,
    Unknown,
}
