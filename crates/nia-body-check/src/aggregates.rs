// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::{BodyChecker, ResolvedEnumSignature, ResolvedStructSignature, ResolvedUnionSignature};
use nia_ast::{Expr, ExprKind, TypeRef};
use nia_comptime_engine::{ComptimeEnv, ComptimeError, ComptimeValue};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
use nia_item_signatures::{EnumSignature, StructSignature};
use nia_span::Span;
use nia_ty::{ArrayLenTy, TyKind};

impl<'a> BodyChecker<'a> {
    pub(crate) fn infer_array_literal_expr(&mut self, expr: &Expr) -> InternedTyId {
        let (ExprKind::ArrayLiteral { elems } | ExprKind::TypedArrayLiteral { elems, .. }) =
            &expr.kind
        else {
            return self.check_expr(expr);
        };
        let ty = self.infer_array_literal_type(expr.span, elems);
        self.record_expr_type(expr.span, ty);
        ty
    }

    pub(crate) fn check_array_literal(
        &mut self,
        span: Span,
        expected: Option<InternedTyId>,
        elems: &nia_ast::ArrayElements,
    ) -> InternedTyId {
        let Some(array_ty) = expected else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "array literal requires an expected array type; add a type annotation",
            ));
            for elem in array_literal_values(elems) {
                self.check_expr(elem);
            }
            return self.error();
        };
        let (len, elem_ty) = match self.interner.get(array_ty) {
            Some(TyKind::Array { len, elem }) => (len.clone(), *elem),
            Some(TyKind::Error) | None => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "array literal type is not an array",
                ));
                return self.error();
            }
        };
        for elem in array_literal_values(elems) {
            let actual = self.check_expr_with_expected(elem, Some(elem_ty));
            self.expect_expr_type(elem, elem_ty, actual, "array literal element");
        }
        self.check_array_literal_len(span, len, elem_ty, elems)
    }

    fn infer_array_literal_type(
        &mut self,
        span: Span,
        elems: &nia_ast::ArrayElements,
    ) -> InternedTyId {
        match elems {
            nia_ast::ArrayElements::List(values) => {
                let Some(anchor_index) = self.array_literal_anchor_index(values) else {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "empty array literal requires an element type annotation",
                    ));
                    return self.error();
                };
                let elem_ty = self.infer_array_literal_elem_type(&values[anchor_index]);
                for (index, elem) in values.iter().enumerate() {
                    if index == anchor_index {
                        continue;
                    }
                    let actual = self.check_expr_with_expected(elem, Some(elem_ty));
                    self.expect_expr_type(elem, elem_ty, actual, "array literal element");
                }
                self.check_array_literal_len(span, ArrayLenTy::Infer, elem_ty, elems)
            }
            nia_ast::ArrayElements::Repeat { value, .. } => {
                let elem_ty = self.infer_array_literal_elem_type(value);
                self.check_array_literal_len(span, ArrayLenTy::Infer, elem_ty, elems)
            }
        }
    }

    fn array_literal_anchor_index(&self, elems: &[Expr]) -> Option<usize> {
        elems
            .iter()
            .position(|elem| !self.is_numeric_literal_expr(elem))
            .or((!elems.is_empty()).then_some(0))
    }

    fn infer_array_literal_elem_type(&mut self, elem: &Expr) -> InternedTyId {
        if matches!(
            elem.kind,
            ExprKind::ArrayLiteral { .. } | ExprKind::TypedArrayLiteral { .. }
        ) {
            self.infer_array_literal_expr(elem)
        } else {
            self.check_expr(elem)
        }
    }

    fn check_array_literal_len(
        &mut self,
        span: Span,
        len: ArrayLenTy,
        elem_ty: InternedTyId,
        elems: &nia_ast::ArrayElements,
    ) -> InternedTyId {
        match len {
            ArrayLenTy::Infer => {
                let inferred = match elems {
                    nia_ast::ArrayElements::List(elems) => elems.len() as u64,
                    nia_ast::ArrayElements::Repeat { count, .. } => {
                        match self.eval_array_repeat_count(count) {
                            Ok(value) => value,
                            Err(err) => {
                                self.diagnostics.push(Diagnostic::error(
                                    err.span,
                                    format!(
                                        "array repeat count is not a valid constant: {}",
                                        err.message
                                    ),
                                ));
                                0
                            }
                        }
                    }
                };
                self.interner.intern(TyKind::Array {
                    len: ArrayLenTy::ConstValue(inferred),
                    elem: elem_ty,
                })
            }
            expected @ (ArrayLenTy::ConstValue(_)
            | ArrayLenTy::ConstExpr(_)
            | ArrayLenTy::Builtin { .. }) => {
                match explicit_array_literal_len(self, elems) {
                    Ok(Some(actual)) => match self.array_len_value(span, &expected) {
                        Ok(expected) => {
                            if expected != actual {
                                self.diagnostics.push(Diagnostic::error(
                                    span,
                                    format!(
                                        "array literal length mismatch: expected {expected}, got {actual}"
                                    ),
                                ));
                            }
                        }
                        Err(err) => self.diagnostics.push(Diagnostic::error(
                            span,
                            format!("array length is not a valid constant: {err}"),
                        )),
                    },
                    Ok(None) => {}
                    Err(err) => {
                        self.diagnostics.push(Diagnostic::error(
                            err.span,
                            format!(
                                "array repeat count is not a valid constant: {}",
                                err.message
                            ),
                        ));
                    }
                }
                self.interner.intern(TyKind::Array {
                    len: expected,
                    elem: elem_ty,
                })
            }
        }
    }

    pub(crate) fn check_struct_literal(
        &mut self,
        span: Span,
        expected: Option<InternedTyId>,
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        let Some(aggregate_ty) = expected else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "aggregate literal requires an expected struct or union type; add a type annotation",
            ));
            for field in fields {
                self.check_expr(&field.value);
            }
            return self.error();
        };
        let (def_id, args) = match self.interner.get(aggregate_ty) {
            Some(TyKind::Nominal { def_id, args }) => (*def_id, args.clone()),
            Some(TyKind::Error) | None => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "aggregate literal type is not nominal",
                ));
                return self.error();
            }
        };
        if self.is_union_def(def_id) {
            return self.check_union_literal(span, aggregate_ty, def_id, &args, fields);
        }
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "struct signature not found"));
            return self.error();
        };
        let generics = resolved.signature.generics.clone();
        let signature_fields = resolved.signature.fields.clone();
        let substitutions = self.generic_substitutions(&generics, &args);
        let field_tys: HashMap<&str, InternedTyId> = signature_fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    self.substitute_generics(field.ty, &substitutions),
                )
            })
            .collect();
        let mut seen_fields = HashSet::new();
        for field in fields {
            if !seen_fields.insert(field.name.as_str()) {
                self.diagnostics.push(Diagnostic::error(
                    field.span,
                    format!("duplicate struct field `{}`", field.name),
                ));
            }
            if let Some(expected) = field_tys.get(field.name.as_str()).copied() {
                let actual = self.check_expr_with_expected(&field.value, Some(expected));
                self.expect_expr_type(&field.value, expected, actual, "struct literal field");
            } else {
                self.check_expr(&field.value);
                self.diagnostics.push(Diagnostic::error(
                    field.span,
                    format!("unknown struct field `{}`", field.name),
                ));
            }
        }
        for field in &signature_fields {
            if !seen_fields.contains(field.name.as_str()) {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("missing struct field `{}`", field.name),
                ));
            }
        }
        aggregate_ty
    }

    fn check_union_literal(
        &mut self,
        span: Span,
        union_ty: InternedTyId,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        let Some(resolved) = self.resolved_union_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "union signature not found"));
            return self.error();
        };
        if fields.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "union literal requires exactly one field, got {}",
                    fields.len()
                ),
            ));
            for field in fields {
                self.check_expr(&field.value);
            }
            return union_ty;
        }
        let field = &fields[0];
        let generics = resolved.signature.generics.clone();
        let signature_fields = resolved.signature.fields.clone();
        let substitutions = self.generic_substitutions(&generics, args);
        let Some(signature_field) = signature_fields
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            self.check_expr(&field.value);
            self.diagnostics.push(Diagnostic::error(
                field.span,
                format!("unknown union field `{}`", field.name),
            ));
            return union_ty;
        };
        let expected = self.substitute_generics(signature_field.ty, &substitutions);
        let actual = self.check_expr_with_expected(&field.value, Some(expected));
        self.expect_expr_type(&field.value, expected, actual, "union literal field");
        union_ty
    }

    pub(crate) fn check_field_access(
        &mut self,
        span: Span,
        lhs: &Expr,
        name: &str,
    ) -> InternedTyId {
        if self.values.qualified_values.contains_key(&span) {
            return self
                .qualified_global_type(span)
                .unwrap_or_else(|| self.error());
        }
        if let Some(ty) = self.check_enum_variant_access(span, lhs, name) {
            return ty;
        }
        let lhs_ty = self.check_expr(lhs);
        self.field_access_type_from_lhs_ty(span, lhs_ty, name)
    }

    pub(crate) fn field_access_type_from_lhs_ty(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        name: &str,
    ) -> InternedTyId {
        let Some((def_id, args)) = self.field_base_type(lhs_ty) else {
            if lhs_ty != self.error() {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "field access base is not a struct or union value or pointer",
                ));
            }
            return self.error();
        };
        if self.is_union_def(def_id) {
            return self.check_union_field_access(span, def_id, &args, name);
        }
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "struct signature not found"));
            return self.error();
        };
        let generics = resolved.signature.generics.clone();
        let fields = resolved.signature.fields.clone();
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown struct field `{name}`"),
            ));
            return self.error();
        };
        let substitutions = self.generic_substitutions(&generics, &args);
        self.substitute_generics(field.ty, &substitutions)
    }

    fn check_union_field_access(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        name: &str,
    ) -> InternedTyId {
        let Some(resolved) = self.resolved_union_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "union signature not found"));
            return self.error();
        };
        let generics = resolved.signature.generics.clone();
        let fields = resolved.signature.fields.clone();
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown union field `{name}`"),
            ));
            return self.error();
        };
        let substitutions = self.generic_substitutions(&generics, args);
        self.substitute_generics(field.ty, &substitutions)
    }

    pub(crate) fn qualified_global_type(&mut self, span: Span) -> Option<InternedTyId> {
        let def_id = self.values.qualified_values.get(&span).copied()?;
        if def_id.module_id == self.defs.module_id {
            return self
                .global_types
                .get(&def_id.def_id)
                .or_else(|| self.comptime_types.get(&def_id.def_id))
                .copied();
        }
        if let Some(program_signature) = self.program_comptimes.get(&def_id).cloned() {
            let ty = program_signature
                .signature
                .explicit_type
                .unwrap_or_else(|| self.error());
            return Some(self.import_type_from(&program_signature.interner, ty));
        }
        let program_signature = self.program_globals.get(&def_id)?.clone();
        let ty = program_signature
            .signature
            .explicit_type
            .unwrap_or_else(|| self.error());
        Some(self.import_type_from(&program_signature.interner, ty))
    }

    pub(crate) fn resolved_struct_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedStructSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.structs.get(&def_id.def_id)?.clone();
            return Some(ResolvedStructSignature { signature });
        }
        let program_signature = self.program_structs.get(&def_id)?.clone();
        let signature = StructSignature {
            generics: program_signature.signature.generics,
            fields: program_signature
                .signature
                .fields
                .into_iter()
                .map(|field| nia_item_signatures::FieldSignature {
                    def_id: field.def_id,
                    name: field.name,
                    ty: self.import_type_from(&program_signature.interner, field.ty),
                    span: field.span,
                })
                .collect(),
            is_extern: program_signature.signature.is_extern,
            span: program_signature.signature.span,
        };
        Some(ResolvedStructSignature { signature })
    }

    pub(crate) fn resolved_union_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedUnionSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.unions.get(&def_id.def_id)?.clone();
            return Some(ResolvedUnionSignature { signature });
        }
        let program_signature = self.program_unions.get(&def_id)?.clone();
        let signature = nia_item_signatures::UnionSignature {
            generics: program_signature.signature.generics,
            fields: program_signature
                .signature
                .fields
                .into_iter()
                .map(|field| nia_item_signatures::FieldSignature {
                    def_id: field.def_id,
                    name: field.name,
                    ty: self.import_type_from(&program_signature.interner, field.ty),
                    span: field.span,
                })
                .collect(),
            is_extern: program_signature.signature.is_extern,
            span: program_signature.signature.span,
        };
        Some(ResolvedUnionSignature { signature })
    }

    pub(crate) fn resolved_enum_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedEnumSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.enums.get(&def_id.def_id)?.clone();
            return Some(ResolvedEnumSignature { signature });
        }
        let program_signature = self.program_enums.get(&def_id)?.clone();
        let signature = EnumSignature {
            backing_type: self.import_type_from(
                &program_signature.interner,
                program_signature.signature.backing_type,
            ),
            is_open: program_signature.signature.is_open,
            variants: program_signature.signature.variants,
            span: program_signature.signature.span,
        };
        Some(ResolvedEnumSignature { signature })
    }

    pub(crate) fn check_enum_variant_access(
        &mut self,
        span: Span,
        lhs: &Expr,
        name: &str,
    ) -> Option<InternedTyId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let Some(variants) = self.enum_variant_scope(enum_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "enum member scope not found"));
            return Some(self.error());
        };
        if !variants
            .iter()
            .any(|(variant_name, _)| variant_name == name)
        {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown enum variant `{name}`"),
            ));
            return Some(self.error());
        }
        Some(self.interner.intern(TyKind::Nominal {
            def_id: enum_id,
            args: Vec::new(),
        }))
    }

    pub(crate) fn enum_global_def_id(&self, ty: InternedTyId) -> Option<GlobalDefId> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return None;
        };
        let def_id = *def_id;
        if self.is_enum_def(def_id) {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn is_enum_def(&self, enum_id: GlobalDefId) -> bool {
        if enum_id.module_id == self.defs.module_id {
            self.signatures.enums.contains_key(&enum_id.def_id)
        } else {
            self.program_enums.contains_key(&enum_id)
        }
    }

    pub(crate) fn is_union_def(&self, union_id: GlobalDefId) -> bool {
        if union_id.module_id == self.defs.module_id {
            self.signatures.unions.contains_key(&union_id.def_id)
        } else {
            self.program_unions.contains_key(&union_id)
        }
    }

    pub(crate) fn enum_variant_scope(&self, enum_id: GlobalDefId) -> Option<Vec<(String, DefId)>> {
        let target_defs = self.defs_for_module(enum_id.module_id)?;
        let scope = target_defs.scopes.enum_members.get(&enum_id.def_id)?;
        Some(
            scope
                .variants
                .entries()
                .map(|(name, def_id)| (name.to_string(), def_id))
                .collect(),
        )
    }

    pub(crate) fn enum_variant_info(&mut self, expr: &Expr) -> Option<(GlobalDefId, DefId)> {
        let ExprKind::Qualified { lhs, name } = &expr.kind else {
            return None;
        };
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some((enum_id, variant_id))
    }

    pub(crate) fn check_enum_switch_exhaustive(
        &mut self,
        span: Span,
        enum_id: GlobalDefId,
        has_default: bool,
        covered_variants: &HashSet<DefId>,
    ) {
        if has_default {
            return;
        }
        let Some(resolved) = self.resolved_enum_signature(enum_id) else {
            return;
        };
        if resolved.signature.is_open {
            self.diagnostics.push(Diagnostic::error(
                span,
                "non-exhaustive open enum switch, missing `_`",
            ));
            return;
        }
        let names_and_defs: Vec<(String, DefId)> = resolved
            .signature
            .variants
            .iter()
            .map(|variant| (variant.name.clone(), variant.def_id))
            .collect();
        let missing: Vec<&str> = names_and_defs
            .iter()
            .filter(|(_, def_id)| !covered_variants.contains(def_id))
            .map(|(name, _)| name.as_str())
            .collect();
        if !missing.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "non-exhaustive enum switch, missing: {}",
                    missing.join(", ")
                ),
            ));
        }
    }

    fn field_base_type(&self, ty: InternedTyId) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.field_base_type(*elem),
            _ => None,
        }
    }
}

impl ComptimeEnv for BodyChecker<'_> {
    fn resolve_ident(&mut self, span: Span, name: &str) -> Result<ComptimeValue, ComptimeError> {
        if let Some(local_id) = self.local_comptime_use(span) {
            return self
                .comptime
                .values
                .get(&nia_comptime_check::ComptimeKey::Local(local_id))
                .cloned()
                .ok_or_else(|| ComptimeError {
                    span,
                    message: format!("failed to evaluate comptime value `{name}`"),
                });
        }
        if let Some(global_id) = self.global_comptime_use(span) {
            return self
                .global_comptime_value(global_id)
                .ok_or_else(|| ComptimeError {
                    span,
                    message: format!("failed to evaluate comptime value `{name}`"),
                });
        }
        Err(ComptimeError {
            span,
            message: format!("comptime expression can only use comptime bindings: `{name}`"),
        })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        ty: &TypeRef,
    ) -> Result<ComptimeValue, ComptimeError> {
        let ty_id = self.ty_for_span(ty.span);
        let Some(layout) = self.layout_of(ty_id) else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot compute layout for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        let value = match builtin {
            LayoutBuiltin::Size => layout.size,
            LayoutBuiltin::Align => layout.align,
        };
        Ok(ComptimeValue::Int(value as i128))
    }
}

impl<'a> BodyChecker<'a> {
    fn eval_array_repeat_count(&mut self, count: &Expr) -> Result<u64, ComptimeError> {
        self.check_expr(count);
        nia_comptime_engine::eval_array_len_expr(count, self)
    }

    pub(crate) fn local_comptime_use(&self, span: Span) -> Option<LocalId> {
        let Some(nia_local_resolve::LocalUse::Local(local_id)) = self.locals.uses.get(&span) else {
            return None;
        };
        let local = self.locals.locals.get(*local_id)?;
        (local.kind == nia_local_resolve::LocalKind::ComptimeBinding).then_some(*local_id)
    }

    pub(crate) fn global_comptime_use(&self, span: Span) -> Option<GlobalDefId> {
        if let Some(global_id) = self.values.qualified_values.get(&span).copied() {
            if self.global_def_kind(global_id) == Some(DefKind::Comptime) {
                return Some(global_id);
            }
            return None;
        }
        let Some(nia_value_resolve::ValueNameResolution::Def(def_id)) =
            self.values.names.get(&span)
        else {
            return None;
        };
        let def = self.defs.defs.get(*def_id)?;
        (def.kind == DefKind::Comptime).then_some(self.global_def_id(*def_id))
    }

    pub(crate) fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.defs_for_module(global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id))
            .map(|def| def.kind)
    }
}

fn array_literal_values(elems: &nia_ast::ArrayElements) -> Vec<&Expr> {
    match elems {
        nia_ast::ArrayElements::List(elems) => elems.iter().collect(),
        nia_ast::ArrayElements::Repeat { value, .. } => vec![value],
    }
}

fn explicit_array_literal_len(
    checker: &mut BodyChecker<'_>,
    elems: &nia_ast::ArrayElements,
) -> Result<Option<u64>, ComptimeError> {
    Ok(match elems {
        nia_ast::ArrayElements::List(elems) => Some(elems.len() as u64),
        nia_ast::ArrayElements::Repeat { count, .. } => {
            Some(checker.eval_array_repeat_count(count)?)
        }
    })
}
