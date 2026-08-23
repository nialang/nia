// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::{BodyChecker, generic_inst_base};
use nia_ast::{Expr, ExprKind};
use nia_const_check::{ConstKey, ConstValueType};
use nia_const_eval::{ConstCommonEnv, ConstError, ConstValue, ResolvedConstEnv};
use nia_const_ir::{
    ConstNameResolution, ResolvedConstAssignTarget, ResolvedConstAssignTargetKind,
    ResolvedConstBinding, ResolvedConstExpr, ResolvedConstParam, ResolvedConstTypeArg,
};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
use nia_item_signatures::{
    EnumSignature, EnumVariantPayloadSignature, EnumVariantSignature, StructSignature,
    UnionSignature,
};
use nia_local_resolve::LocalKind;
use nia_sema::{
    ArrayLiteralLenCheck, NamedField, check_array_literal_len, check_required_field_set,
};
use nia_sema_ir::{
    AssociatedConstProjection, BuiltinAssociatedValue, SemanticUseTable, SemanticValueUse,
};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_symbol::SymbolMap;
use nia_ty::{ArrayLenTy, TyKind};

#[derive(Debug, Clone)]
pub(super) struct ResolvedStructSignature {
    pub(super) signature: StructSignature,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedUnionSignature {
    pub(super) signature: UnionSignature,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedEnumSignature {
    pub(super) signature: EnumSignature,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn infer_array_literal_expr(&mut self, expr: &Expr) -> InternedTyId {
        let ExprKind::ArrayLiteral { elems } = &expr.kind else {
            return self.check_expr(expr);
        };
        let ty = self.infer_array_literal_type(expr.span, elems);
        self.record_expr_node_type(expr, ty);
        ty
    }

    pub(crate) fn check_array_literal(
        &mut self,
        span: Span,
        expected: Option<InternedTyId>,
        elems: &nia_ast::ArrayElements,
    ) -> InternedTyId {
        let Some(array_ty) = expected else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "array literal requires an expected array type; add a type annotation",
            ));
            for elem in array_literal_values(elems) {
                self.check_expr(elem);
            }
            return self.error();
        };
        let (len, elem_ty) = match self.expect_ty_kind(array_ty) {
            TyKind::Array { len, elem } => (len.clone(), *elem),
            TyKind::Error => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
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
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
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
        // Unsuffixed numbers are deliberately the last resort: choosing one
        // first would default `[1, 2i64]` to `i32` before the explicit `i64`
        // constraint is seen. Expressions that intrinsically require an
        // expected type are likewise checked only after a concrete peer has
        // established the shared element type.
        elems
            .iter()
            .position(|elem| {
                !self.is_untyped_numeric_literal_expr(elem)
                    && !array_literal_elem_requires_expected(elem)
            })
            .or_else(|| {
                elems
                    .iter()
                    .position(|elem| self.is_untyped_numeric_literal_expr(elem))
            })
    }

    fn infer_array_literal_elem_type(&mut self, elem: &Expr) -> InternedTyId {
        if matches!(elem.kind, ExprKind::ArrayLiteral { .. }) {
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
                let inferred = match explicit_array_literal_len(self, span, elems) {
                    Ok(Some(value)) => {
                        if let nia_ast::ArrayElements::Repeat { count, .. } = elems {
                            self.record_array_repeat_count(count, value);
                        }
                        value
                    }
                    Ok(None) => 0,
                    Err(err) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            err.span,
                            format!(
                                "array literal length is not a valid constant: {}",
                                err.message
                            ),
                        ));
                        0
                    }
                };
                self.interner.intern(TyKind::Array {
                    len: ArrayLenTy::ConstValue(inferred),
                    elem: elem_ty,
                })
            }
            expected @ ArrayLenTy::GenericParam(_) => {
                if matches!(elems, nia_ast::ArrayElements::List(_)) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        "array list length cannot be checked against an unresolved const generic",
                    ));
                }
                self.interner.intern(TyKind::Array {
                    len: expected,
                    elem: elem_ty,
                })
            }
            expected @ (ArrayLenTy::ConstValue(_)
            | ArrayLenTy::ConstExpr(_)
            | ArrayLenTy::Builtin { .. }) => {
                match explicit_array_literal_len(self, span, elems) {
                    Ok(Some(actual)) => {
                        if let nia_ast::ArrayElements::Repeat { count, .. } = elems {
                            self.record_array_repeat_count(count, actual);
                        }
                        match self.array_len_value(span, &expected) {
                            Ok(expected_value) => match check_array_literal_len(
                                Some(expected.clone()),
                                Some(expected_value),
                                Some(actual),
                            ) {
                                ArrayLiteralLenCheck::Mismatch { expected, actual } => {
                                    self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                                        span,
                                        format!(
                                            "array literal length mismatch: expected {expected}, got {actual}"
                                        ),
                                    ));
                                }
                                ArrayLiteralLenCheck::Accepted(_)
                                | ArrayLiteralLenCheck::Unknown => {}
                            },
                            Err(err) => self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_CHECK,
                                span,
                                format!("array length is not a valid constant: {err}"),
                            )),
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
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
        aggregate_ty: InternedTyId,
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        let (def_id, args, const_args) = match self.expect_ty_kind(aggregate_ty) {
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => (*def_id, args.clone(), const_args.clone()),
            TyKind::Error => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "aggregate literal type is not nominal",
                ));
                return self.error();
            }
        };
        if self.is_union_def(def_id) {
            return self.check_union_literal(
                span,
                aggregate_ty,
                def_id,
                &args,
                &const_args,
                fields,
            );
        }
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "struct signature not found",
            ));
            return self.error();
        };
        if resolved.signature.is_tuple {
            for field in fields {
                self.check_expr(&field.value);
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "tuple structs must be constructed with positional arguments",
            ));
            return aggregate_ty;
        }
        let signature_fields = resolved.signature.fields.clone();
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let field_tys: HashMap<SymbolId, InternedTyId> = signature_fields
            .iter()
            .map(|field| {
                (
                    field.name,
                    self.substitute_generics_and_consts(
                        field.ty,
                        &substitutions,
                        &const_substitutions,
                    ),
                )
            })
            .collect();
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span, field.name)),
            signature_fields.iter().map(|field| field.name),
        );
        for field in fields {
            if let Some(expected) = field_tys.get(&field.name).copied() {
                let actual = self.check_expr_with_expected(&field.value, Some(expected));
                self.expect_expr_type(&field.value, expected, actual, "struct literal field");
            } else {
                self.check_expr(&field.value);
            }
        }
        for field in field_set.duplicate_fields {
            let name = self.symbol_name(field.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!("duplicate struct field `{name}`"),
            ));
        }
        for field in field_set.unknown_fields {
            let name = self.symbol_name(field.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!("unknown struct field `{name}`"),
            ));
        }
        for name in field_set.missing_fields {
            let name = self.symbol_name(name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("missing struct field `{name}`"),
            ));
        }
        aggregate_ty
    }

    fn check_union_literal(
        &mut self,
        span: Span,
        union_ty: InternedTyId,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        let Some(resolved) = self.resolved_union_signature(def_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "union signature not found",
            ));
            return self.error();
        };
        if fields.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
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
        let signature_fields = resolved.signature.fields.clone();
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, args, const_args);
        self.check_const_union_representation(
            span,
            &signature_fields,
            &substitutions,
            &const_substitutions,
        );
        let Some(signature_field) = signature_fields
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            self.check_expr(&field.value);
            let name = self.symbol_name(field.name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!("unknown union field `{name}`"),
            ));
            return union_ty;
        };
        let expected = self.substitute_generics_and_consts(
            signature_field.ty,
            &substitutions,
            &const_substitutions,
        );
        let actual = self.check_expr_with_expected(&field.value, Some(expected));
        self.expect_expr_type(&field.value, expected, actual, "union literal field");
        union_ty
    }

    pub(crate) fn check_field_access(
        &mut self,
        expr: &Expr,
        lhs: &Expr,
        name: &SymbolId,
    ) -> InternedTyId {
        let span = expr.span;
        if matches!(
            self.semantic_uses.node_value_use(&expr.node_key),
            Some(SemanticValueUse::Global(_))
        ) {
            return self
                .qualified_global_type(expr)
                .unwrap_or_else(|| self.error());
        }
        if let Some(ty) = self.check_enum_variant_access(span, lhs, name) {
            return ty;
        }
        let lhs_ty = self.check_expr(lhs);
        if matches!(self.interner.get(lhs_ty), Some(TyKind::ConstOnly))
            && let Some(ty) = self.const_field_expr_runtime_type(lhs, name)
        {
            return ty;
        }
        self.field_access_type_from_lhs_ty(span, lhs_ty, name)
    }

    fn const_field_expr_runtime_type(
        &mut self,
        lhs: &Expr,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let expr = ResolvedConstExpr::field(lhs.span, self.lower_const_expr(lhs).ok()?, *name);
        match self.const_expr_type_for_ir_with_expected(&expr, None)? {
            ConstValueType::Runtime(ty) => Some(ty),
            _ => Some(self.interner.intern(TyKind::ConstOnly)),
        }
    }

    pub(crate) fn const_index_expr_runtime_type(
        &mut self,
        lhs: &Expr,
        index: &Expr,
    ) -> Option<InternedTyId> {
        let expr = ResolvedConstExpr::index(
            lhs.span,
            self.lower_const_expr(lhs).ok()?,
            self.lower_const_expr(index).ok()?,
        );
        match self.const_expr_type_for_ir_with_expected(&expr, None)? {
            ConstValueType::Runtime(ty) => Some(ty),
            _ => Some(self.interner.intern(TyKind::ConstOnly)),
        }
    }

    pub(crate) fn const_slice_expr_runtime_type(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expr = self.lower_const_expr(expr).ok()?;
        match self.const_expr_type_for_ir_with_expected(&expr, expected)? {
            ConstValueType::Runtime(ty) => Some(ty),
            _ => Some(self.interner.intern(TyKind::ConstOnly)),
        }
    }

    pub(crate) fn field_access_type_from_lhs_ty(
        &mut self,
        span: Span,
        lhs_ty: InternedTyId,
        name: &SymbolId,
    ) -> InternedTyId {
        let Some((def_id, args, const_args)) = self.field_base_type(lhs_ty) else {
            if matches!(self.interner.get(lhs_ty), Some(TyKind::ConstOnly)) {
                return lhs_ty;
            }
            if lhs_ty != self.error() {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "field access base is not a struct or union value or pointer",
                ));
            }
            return self.error();
        };
        if self.is_union_def(def_id) {
            return self.check_union_field_access(span, def_id, &args, &const_args, name);
        }
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "struct signature not found",
            ));
            return self.error();
        };
        let fields = resolved.signature.fields.clone();
        let Some(field) = fields.iter().find(|field| &field.name == name) else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("unknown struct field `{name}`"),
            ));
            return self.error();
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions)
    }

    fn check_union_field_access(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        name: &SymbolId,
    ) -> InternedTyId {
        let Some(resolved) = self.resolved_union_signature(def_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "union signature not found",
            ));
            return self.error();
        };
        let fields = resolved.signature.fields.clone();
        let Some(field) = fields.iter().find(|field| &field.name == name) else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("unknown union field `{name}`"),
            ));
            return self.error();
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, args, const_args);
        self.check_const_union_representation(span, &fields, &substitutions, &const_substitutions);
        self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions)
    }

    fn check_const_union_representation(
        &mut self,
        span: Span,
        fields: &[nia_item_signatures::FieldSignature],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    ) {
        for field in fields {
            let ty =
                self.substitute_generics_and_consts(field.ty, substitutions, const_substitutions);
            if self.const_union_ty_has_abi_model(ty) {
                continue;
            }
            self.reject_const_operation(
                span,
                format!(
                    "const union field `{}` requires a supported const ABI type",
                    self.symbol_name(field.name)
                ),
            );
            return;
        }
    }

    fn const_union_ty_has_abi_model(&mut self, ty: InternedTyId) -> bool {
        self.const_union_ty_has_abi_model_inner(ty, &mut HashSet::new())
    }

    fn const_union_ty_has_abi_model_inner(
        &mut self,
        ty: InternedTyId,
        visiting: &mut HashSet<InternedTyId>,
    ) -> bool {
        let ty = self.normalize_aliases(ty);
        if !visiting.insert(ty) {
            return false;
        }
        let supported = match self.expect_ty_kind(ty).clone() {
            TyKind::Primitive(primitive) => !matches!(primitive, nia_ty::PrimitiveTy::Never),
            TyKind::Tuple(elems) => {
                !elems.is_empty()
                    && elems
                        .iter()
                        .all(|elem| self.const_union_ty_has_abi_model_inner(*elem, visiting))
            }
            TyKind::Array { elem, .. } => self.const_union_ty_has_abi_model_inner(elem, visiting),
            TyKind::Vector { elem, lanes } => {
                elem.is_vector_element()
                    && lanes > 0
                    && nia_layout::vector_layout(elem, lanes, self.layouts.target).is_some()
            }
            TyKind::Pointer { .. } => true,
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                let fields = if let Some(resolved) = self.resolved_struct_signature(def_id) {
                    resolved.signature.fields.clone()
                } else if let Some(resolved) = self.resolved_union_signature(def_id) {
                    resolved.signature.fields.clone()
                } else {
                    visiting.remove(&ty);
                    return false;
                };
                let (substitutions, const_substitutions) =
                    self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
                fields.iter().all(|field| {
                    let field_ty = self.substitute_generics_and_consts(
                        field.ty,
                        &substitutions,
                        &const_substitutions,
                    );
                    self.const_union_ty_has_abi_model_inner(field_ty, visiting)
                })
            }
            TyKind::GenericParam(_) => true,
            _ => false,
        };
        visiting.remove(&ty);
        supported
    }

    pub(crate) fn qualified_global_type(&mut self, expr: &Expr) -> Option<InternedTyId> {
        let Some(SemanticValueUse::Global(def_id)) =
            self.semantic_uses.node_value_use(&expr.node_key)
        else {
            return None;
        };
        if def_id.module_id == self.defs.module_id {
            return self
                .global_types
                .get(&def_id.def_id)
                .or_else(|| self.const_types.get(&def_id.def_id))
                .copied();
        }
        if let Some(ty) = self.qualified_program_const_type(def_id) {
            return Some(ty);
        }
        let program_signature = self.program_signature_scope.global(def_id)?;
        let ty = program_signature
            .signature
            .explicit_type
            .unwrap_or_else(|| self.error());
        Some(ty)
    }

    pub(crate) fn qualified_program_const_type(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        let program_signature = self.program_signature_scope.const_eval(def_id)?;
        if let Some(ty) = program_signature.signature.explicit_type {
            return Some(ty);
        }
        if let Some(typed) = (self.program_const_values)(def_id.module_id).and_then(|const_eval| {
            const_eval
                .typed_values
                .get(&ConstKey::Global(def_id))
                .cloned()
        }) && let Some(ty) = self.const_value_runtime_type(typed.ty)
            && ty != self.error()
        {
            return Some(ty);
        }
        Some(self.error())
    }

    fn const_value_runtime_type(&self, ty: ConstValueType) -> Option<InternedTyId> {
        let ConstValueType::Runtime(ty) = ty else {
            return None;
        };
        self.type_store.get(ty).map(|_| ty)
    }

    pub(crate) fn resolved_struct_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedStructSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.structs.get(&def_id.def_id)?.clone();
            return Some(ResolvedStructSignature { signature });
        }
        let program_signature = self.program_signature_scope.struct_(def_id)?;
        let signature = program_signature.signature;
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
        let program_signature = self.program_signature_scope.union(def_id)?;
        let signature = program_signature.signature;
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
        let program_signature = self.program_signature_scope.enum_(def_id)?;
        let signature = program_signature.signature;
        Some(ResolvedEnumSignature { signature })
    }

    pub(crate) fn check_enum_variant_access(
        &mut self,
        span: Span,
        lhs: &Expr,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let Some(variants) = self.enum_variant_scope(enum_id) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "enum member scope not found",
            ));
            return Some(self.error());
        };
        let Some((_, variant_def)) = variants
            .iter()
            .find(|(variant_name, _)| variant_name == name)
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("unknown enum variant `{}`", self.symbol_name(*name)),
            ));
            return Some(self.error());
        };
        let variant_id = GlobalDefId {
            module_id: enum_id.module_id,
            def_id: *variant_def,
        };
        if let Some((_, variant)) = self.resolved_enum_variant(variant_id)
            && !matches!(variant.payload, EnumVariantPayloadSignature::Unit)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "enum variant `{}` requires a payload",
                    self.symbol_name(variant.name)
                ),
            ));
        }
        Some(self.enum_ty(enum_id))
    }

    pub(crate) fn resolved_enum_variant(
        &mut self,
        variant_id: GlobalDefId,
    ) -> Option<(GlobalDefId, EnumVariantSignature)> {
        let defs = self.defs_for_module(variant_id.module_id)?;
        let enum_def = defs.as_ref().defs.get(variant_id.def_id)?.parent?;
        let enum_id = GlobalDefId {
            module_id: variant_id.module_id,
            def_id: enum_def,
        };
        let signature = self.resolved_enum_signature(enum_id)?.signature;
        let variant = signature
            .variants
            .into_iter()
            .find(|variant| variant.def_id == variant_id.def_id)?;
        Some((enum_id, variant))
    }

    pub(crate) fn check_enum_variant_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
    ) -> Option<InternedTyId> {
        let (enum_id, variant_def) = self.enum_variant_info(callee)?;
        let variant_id = GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_def,
        };
        let (_, variant) = self.resolved_enum_variant(variant_id)?;
        if let EnumVariantPayloadSignature::Tuple(fields) = &variant.payload {
            self.check_enum_tuple_payload(expr.span, &variant, fields, args);
            return Some(self.enum_ty(enum_id));
        }
        for arg in args {
            self.check_expr(arg);
        }
        let expected = if matches!(variant.payload, EnumVariantPayloadSignature::Unit) {
            "no payload"
        } else {
            "a named payload literal"
        };
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            expr.span,
            format!(
                "enum variant `{}` expects {expected}",
                self.symbol_name(variant.name)
            ),
        ));
        Some(self.enum_ty(enum_id))
    }

    /// Type-checks a positional struct constructor while preserving nominal identity.
    ///
    /// Tuple structs share storage and field identities with named structs, but their
    /// source-level constructor is deliberately call-shaped. Recognizing it before the
    /// ordinary call checker prevents a type name from being treated as a function value.
    pub(crate) fn check_tuple_struct_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
    ) -> Option<InternedTyId> {
        let (def_id, type_args, const_args) = self.type_prefix_instance(callee)?;
        let resolved = self.resolved_struct_signature(def_id)?;
        if !resolved.signature.is_tuple {
            return None;
        }
        let ty = self.interner.intern(TyKind::Nominal {
            def_id,
            args: type_args.clone(),
            const_args: const_args.clone(),
        });
        let fields = resolved.signature.fields;
        if fields.len() != args.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "tuple struct expects {} constructor arguments, found {}",
                    fields.len(),
                    args.len()
                ),
            ));
        }
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &type_args, &const_args);
        for (index, arg) in args.iter().enumerate() {
            let Some(field) = fields.get(index) else {
                self.check_expr(arg);
                continue;
            };
            let expected =
                self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions);
            let actual = self.check_expr_with_expected(arg, Some(expected));
            self.expect_expr_type(arg, expected, actual, "tuple struct constructor argument");
        }
        Some(ty)
    }

    fn check_enum_tuple_payload(
        &mut self,
        span: Span,
        variant: &EnumVariantSignature,
        fields: &[InternedTyId],
        args: &[Expr],
    ) {
        if fields.len() != args.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "enum variant `{}` expects {} payload values, found {}",
                    self.symbol_name(variant.name),
                    fields.len(),
                    args.len()
                ),
            ));
        }
        for (index, arg) in args.iter().enumerate() {
            let Some(expected) = fields.get(index).copied() else {
                self.check_expr(arg);
                continue;
            };
            let actual = self.check_expr_with_expected(arg, Some(expected));
            self.expect_expr_type(arg, expected, actual, "enum variant payload");
        }
    }

    pub(crate) fn check_qualified_struct_literal(
        &mut self,
        expr: &Expr,
        target: &Expr,
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        if let Some((enum_id, variant_def)) = self.enum_variant_info(target) {
            let variant_id = GlobalDefId {
                module_id: enum_id.module_id,
                def_id: variant_def,
            };
            let Some((_, variant)) = self.resolved_enum_variant(variant_id) else {
                return self.error();
            };
            let EnumVariantPayloadSignature::Named(expected_fields) = &variant.payload else {
                for field in fields {
                    self.check_expr(&field.value);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!(
                        "enum variant `{}` does not have a named payload",
                        self.symbol_name(variant.name)
                    ),
                ));
                return self.enum_ty(enum_id);
            };
            self.check_named_enum_payload(expr.span, &variant, expected_fields, fields);
            return self.enum_ty(enum_id);
        }
        if let Some((def_id, args, const_args)) = self.type_prefix_instance(target) {
            let ty = self.interner.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            });
            return self.check_struct_literal(expr.span, ty, fields);
        }
        for field in fields {
            self.check_expr(&field.value);
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            expr.span,
            "qualified literal target is not a struct or named enum variant",
        ));
        self.error()
    }

    fn check_named_enum_payload(
        &mut self,
        span: Span,
        variant: &EnumVariantSignature,
        expected_fields: &[nia_item_signatures::FieldSignature],
        fields: &[nia_ast::FieldInit],
    ) {
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span, field.name)),
            expected_fields.iter().map(|field| field.name),
        );
        for field in fields {
            if let Some(expected) = expected_fields
                .iter()
                .find(|expected| expected.name == field.name)
                .map(|field| field.ty)
            {
                let actual = self.check_expr_with_expected(&field.value, Some(expected));
                self.expect_expr_type(&field.value, expected, actual, "enum variant payload field");
            } else {
                self.check_expr(&field.value);
            }
        }
        for field in field_set.duplicate_fields {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!("duplicate payload field `{}`", self.symbol_name(field.name)),
            ));
        }
        for field in field_set.unknown_fields {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                field.span,
                format!("unknown payload field `{}`", self.symbol_name(field.name)),
            ));
        }
        for name in field_set.missing_fields {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "missing payload field `{}` for variant `{}`",
                    self.symbol_name(name),
                    self.symbol_name(variant.name)
                ),
            ));
        }
    }

    fn enum_ty(&mut self, enum_id: GlobalDefId) -> InternedTyId {
        self.interner.intern(TyKind::Nominal {
            def_id: enum_id,
            args: Vec::new(),
            const_args: Vec::new(),
        })
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
            self.program_signature_scope.has_enum(enum_id)
        }
    }

    pub(crate) fn is_union_def(&self, union_id: GlobalDefId) -> bool {
        if union_id.module_id == self.defs.module_id {
            self.signatures.unions.contains_key(&union_id.def_id)
        } else {
            self.program_signature_scope.has_union(union_id)
        }
    }

    pub(crate) fn enum_variant_scope(
        &self,
        enum_id: GlobalDefId,
    ) -> Option<Vec<(SymbolId, DefId)>> {
        let target_defs = self.defs_for_module(enum_id.module_id)?;
        let scope = target_defs
            .as_ref()
            .scopes
            .enum_members
            .get(&enum_id.def_id)?;
        Some(
            scope
                .variants
                .entries()
                .map(|(name, def_id)| (*name, def_id))
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

    fn field_base_type(
        &mut self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let ty = self.normalize_aliases_in_type(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => Some((def_id, args, const_args)),
            Some(TyKind::Pointer { elem, .. }) | Some(TyKind::VolatilePointer { elem, .. }) => {
                self.field_base_type(elem)
            }
            _ => None,
        }
    }
}

impl ConstCommonEnv for BodyChecker<'_> {
    fn begin_const_eval(&mut self) {
        self.const_eval_budget.begin_session();
    }

    fn end_const_eval(&mut self) {
        self.const_eval_budget.end_session();
    }

    fn consume_const_eval_step(&mut self, span: Span) -> Result<(), ConstError> {
        self.const_eval_budget.consume_step(span)
    }

    fn is_enum_variant(&self, def_id: GlobalDefId) -> bool {
        self.global_def_kind(def_id) == Some(DefKind::EnumVariant)
    }

    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.const_call_locals
            .push(crate::ConstCallFrame::default());
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.const_call_locals.pop();
    }

    fn push_function_frame(&mut self, span: Span) -> Result<(), ConstError> {
        self.const_eval_budget.enter_call(span)?;
        self.const_call_locals
            .push(crate::ConstCallFrame::default());
        Ok(())
    }

    fn pop_function_frame(&mut self) {
        self.const_call_locals.pop();
        self.const_eval_budget.leave_call();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: nia_ids::ModuleId,
        function_id: Option<GlobalDefId>,
        substitutions: Vec<(SymbolId, InternedTyId)>,
        const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    ) -> Result<(), ConstError> {
        let substitutions = substitutions.into_iter().collect::<Vec<_>>();
        let resolved_const_substitutions = const_substitutions
            .into_iter()
            .map(|(name, arg)| {
                let arg = self.resolve_const_const_generic_arg(arg);
                (name, arg)
            })
            .collect::<Vec<_>>();
        let Some(frame) = self.const_call_locals.last_mut() else {
            return Err(ConstError {
                span,
                message: "failed to bind const function type substitutions".to_string(),
            });
        };
        frame.module_id = Some(module_id);
        frame.function_id = function_id;
        frame.type_substitutions.extend(substitutions);
        frame
            .const_substitutions
            .extend(resolved_const_substitutions);
        Ok(())
    }
}

impl ResolvedConstEnv for BodyChecker<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ConstNameResolution,
    ) -> Result<ConstValue, ConstError> {
        match resolution {
            ConstNameResolution::Local(local_id) => {
                if let Some(value) = self.const_call_local_value(local_id) {
                    return Ok(value);
                }
                self.const_eval
                    .values
                    .get(&nia_const_check::ConstKey::Local(local_id))
                    .cloned()
                    .ok_or_else(|| ConstError {
                        span,
                        message: "failed to evaluate resolved const local".to_string(),
                    })
            }
            ConstNameResolution::Global(global_id) => {
                if self.global_def_kind(global_id) == Some(DefKind::Const) {
                    return self
                        .global_const_value(global_id)
                        .ok_or_else(|| ConstError {
                            span,
                            message: "failed to evaluate resolved const global".to_string(),
                        });
                }
                Err(ConstError {
                    span,
                    message: "resolved const expression can only use const bindings".to_string(),
                })
            }
            ConstNameResolution::GenericParam(name) => self
                .active_const_execution_frames()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .and_then(const_value_from_const_generic_arg)
                .ok_or_else(|| ConstError {
                    span,
                    message: format!(
                        "failed to evaluate const generic parameter `{}`",
                        self.symbol_name(name)
                    ),
                }),
            ConstNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } = value;
                let Some(value) = kind.value(primitive, self.target.pointer_width) else {
                    return Err(ConstError {
                        span,
                        message: "builtin associated value is not representable at const"
                            .to_string(),
                    });
                };
                Ok(ConstValue::Int(value))
            }
            ConstNameResolution::AssociatedConstProjection(projection) => {
                self.resolve_associated_const_projection_for_env(span, projection)
            }
        }
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        let ty_id = self.substitute_current_const_generics(type_arg.ty());
        let Some(layout) = self.layout_of(ty_id) else {
            return Err(ConstError {
                span,
                message: format!(
                    "cannot compute layout for const builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        Ok(ConstValue::Int(nia_ty::IntConst::unsigned(
            layout.builtin_value(builtin) as u128,
        )))
    }

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        generic_args: &[nia_const_ir::ResolvedConstGenericArg],
        arg_exprs: &[ResolvedConstExpr],
        receiver_place: Option<&nia_const_eval::ResolvedConstPlace>,
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let Some(function_id) = self.resolved_const_function(callee) else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        let Some(signature) = self
            .resolved_function_signature(function_id)
            .map(|resolved| resolved.signature)
        else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        let instantiation = self.instantiate_resolved_const_function_generics(
            span,
            function_id,
            &signature,
            generic_args,
            arg_exprs,
        )?;
        let Some(function) = self.const_function_body(function_id) else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        let output = nia_const_eval::eval_resolved_const_function_call(
            nia_const_eval::ResolvedConstCallInput {
                span,
                function_id,
                function_module_id: function_id.module_id,
                function: &function,
                type_substitutions: instantiation.type_substitutions.into_iter().collect(),
                const_substitutions: instantiation.const_substitutions.into_iter().collect(),
                args,
            },
            self,
        )?;
        if let Some(receiver) = output.mutable_receiver {
            let Some(receiver_place) = receiver_place else {
                return Err(ConstError {
                    span,
                    message: "mutable const receiver requires a place".to_string(),
                });
            };
            nia_const_eval::write_resolved_const_place(span, receiver_place, receiver, self)?;
        }
        Ok(output.value)
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedConstParam,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = param.ty().map(|ty| {
            nia_const_check::ConstValueType::Runtime(self.substitute_current_const_generics(ty))
        });
        self.bind_const_call_local_value(
            span,
            param.local_id(),
            param.receiver() == Some(nia_ids::ReceiverKind::Ref),
            value,
            ty,
        )
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedConstBinding,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = binding
            .explicit_type()
            .map(|ty| {
                nia_const_check::ConstValueType::Runtime(self.substitute_current_const_generics(ty))
            })
            .or_else(|| self.const_expr_type_for_ir_with_expected(binding.value(), None));
        self.bind_const_call_local_value(span, binding.local_id(), binding.is_mutable(), value, ty)
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        _name: &SymbolId,
        local_id: LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = self
            .local_types
            .get(&local_id)
            .copied()
            .map(nia_const_check::ConstValueType::Runtime);
        self.bind_const_call_local_value(span, local_id, false, value, ty)
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedConstAssignTarget,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        match target.kind() {
            ResolvedConstAssignTargetKind::Local { name, local_id, .. } => {
                self.assign_const_call_local_value(span, *local_id, Some(name), value)
            }
        }
    }

    fn assign_resolved_place_local(
        &mut self,
        span: Span,
        local_id: LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        self.assign_const_call_local_value(span, local_id, None, value)
    }
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn lower_const_expr(
        &self,
        expr: &Expr,
    ) -> Result<nia_const_ir::ResolvedConstExpr, nia_const_ir::ConstLowerError> {
        let semantic_uses = self.const_semantic_uses();
        let context =
            nia_const_ir::ResolvedConstLowerInputs::new(&semantic_uses).with_symbols(self.symbols);
        nia_const_ir::lower_expr_resolved_with_context(expr, &context)
    }

    fn const_semantic_uses(&self) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, value_use) in &self.semantic_uses.node_value_uses {
            match value_use {
                SemanticValueUse::Local(local_id)
                    if self
                        .active_const_execution_frames()
                        .any(|frame| frame.locals.contains_key(local_id))
                        || self
                            .locals
                            .locals
                            .get(*local_id)
                            .is_some_and(|local| local.kind == LocalKind::ConstBinding) =>
                {
                    builder.insert_node_local_value_use(key.clone(), *local_id);
                }
                SemanticValueUse::Global(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                SemanticValueUse::Local(_) => {}
            }
        }
        builder.extend_node_local_defs(
            self.semantic_uses
                .node_local_defs
                .iter()
                .map(|(key, local_id)| (key.clone(), *local_id)),
        );
        builder.extend_node_const_generic_uses(
            self.semantic_uses
                .node_const_generic_uses
                .iter()
                .map(|(key, name)| (key.clone(), *name)),
        );
        builder.extend_node_type_uses(
            self.semantic_uses
                .node_type_uses
                .iter()
                .map(|(key, ty)| (key.clone(), *ty)),
        );
        builder.extend_node_type_prefixes(
            self.semantic_uses
                .node_type_prefixes
                .iter()
                .map(|(key, def_id)| (key.clone(), *def_id)),
        );
        builder.finish()
    }

    pub(crate) fn eval_array_repeat_count(&mut self, count: &Expr) -> Result<u64, ConstError> {
        self.with_const_context(|this| {
            this.check_expr(count);
            let count = this
                .lower_const_expr(count)
                .map_err(|err| nia_const_eval::ConstError {
                    span: err.span,
                    message: err.message,
                })?;
            nia_const_eval::eval_resolved_const_array_len_expr(&count, this)
        })
    }

    fn instantiate_resolved_const_function_generics(
        &mut self,
        span: Span,
        function_id: GlobalDefId,
        signature: &nia_item_signatures::FunctionSignature,
        generic_args: &[nia_const_ir::ResolvedConstGenericArg],
        arg_exprs: &[ResolvedConstExpr],
    ) -> Result<nia_const_check::ConstGenericInstantiation, ConstError> {
        let frames = self.typed_const_frames();
        let trait_impls_for_module = |_| Some(self.program_trait_impls.to_vec());
        let program_signature_scope = self.program_signature_scope;
        let program_is_enum = |def_id| program_signature_scope.has_enum(def_id);
        nia_const_check::instantiate_resolved_const_function_generics(
            nia_const_check::TypedConstQueryInput {
                type_store: self.type_store,
                module: self.const_module,
                defs: self.defs,
                values: self.values,
                locals: self.locals,
                semantic_uses: self.semantic_uses,
                symbols: self.symbols,
                lowered: self.type_lowering,
                signatures: self.const_signatures,
                normalization: self.normalization,
                target: self.target,
                source_path: self.source_path,
                program: nia_const_check::ConstProgramContext {
                    module: Some(self.program_const_module),
                    source_path: None,
                    defs: self.program.defs,
                    type_normalizations: self.program.type_normalizations,
                    signatures: self.program.signatures,
                    function_signatures: self.program.signatures,
                    value_signatures: self.program.signatures,
                    const_values: Some(self.program_const_values),
                    global_initializer: None,
                    program_is_enum: Some(&program_is_enum),
                    trait_impls_for_module: Some(&trait_impls_for_module),
                    visible_extensions: self.program.visible_extensions,
                },
                typed_values: self.const_eval.typed_values,
                array_lengths: self.const_eval.array_lengths,
                frames: &frames,
            },
            span,
            function_id.module_id,
            signature,
            generic_args,
            arg_exprs,
            None,
        )
    }

    fn typed_const_frames(&self) -> Vec<nia_const_check::TypedConstFrame> {
        let frames = self.active_const_execution_frames().collect::<Vec<_>>();
        frames
            .into_iter()
            .rev()
            .map(|frame| nia_const_check::TypedConstFrame {
                module_id: frame.module_id,
                function_id: frame.function_id,
                local_types: frame.local_types.clone(),
                type_substitutions: frame.type_substitutions.clone(),
                const_substitutions: frame.const_substitutions.clone(),
            })
            .collect()
    }

    pub(crate) fn const_expr_type_for_ir_with_expected(
        &mut self,
        expr: &nia_const_ir::ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<nia_const_check::ConstValueType> {
        let frames = self.typed_const_frames();
        let trait_impls_for_module = |_| Some(self.program_trait_impls.to_vec());
        let program_signature_scope = self.program_signature_scope;
        let program_is_enum = |def_id| program_signature_scope.has_enum(def_id);
        let input = nia_const_check::TypedConstQueryInput {
            type_store: self.type_store,
            module: self.const_module,
            defs: self.defs,
            values: self.values,
            locals: self.locals,
            semantic_uses: self.semantic_uses,
            symbols: self.symbols,
            lowered: self.type_lowering,
            signatures: self.const_signatures,
            normalization: self.normalization,
            target: self.target,
            source_path: self.source_path,
            program: nia_const_check::ConstProgramContext {
                module: Some(self.program_const_module),
                source_path: None,
                defs: self.program.defs,
                type_normalizations: self.program.type_normalizations,
                signatures: self.program.signatures,
                function_signatures: self.program.signatures,
                value_signatures: self.program.signatures,
                const_values: Some(self.program_const_values),
                global_initializer: None,
                program_is_enum: Some(&program_is_enum),
                trait_impls_for_module: Some(&trait_impls_for_module),
                visible_extensions: self.program.visible_extensions,
            },
            typed_values: self.const_eval.typed_values,
            array_lengths: self.const_eval.array_lengths,
            frames: &frames,
        };
        let ty = nia_const_check::infer_resolved_const_expr_type(input, expr, expected)?;
        Some(self.import_current_const_expr_type(ty))
    }

    fn import_current_const_expr_type(
        &mut self,
        ty: nia_const_check::ConstValueType,
    ) -> nia_const_check::ConstValueType {
        match ty {
            nia_const_check::ConstValueType::Runtime(ty) => {
                nia_const_check::ConstValueType::Runtime(ty)
            }
            nia_const_check::ConstValueType::Array { elem, len } => {
                nia_const_check::ConstValueType::Array {
                    elem: Box::new(self.import_current_const_expr_type(*elem)),
                    len,
                }
            }
            nia_const_check::ConstValueType::Int => nia_const_check::ConstValueType::Int,
            nia_const_check::ConstValueType::Bool => nia_const_check::ConstValueType::Bool,
            nia_const_check::ConstValueType::String => nia_const_check::ConstValueType::String,
        }
    }

    fn substitute_current_const_generics(&mut self, ty: InternedTyId) -> InternedTyId {
        let frames = self.active_const_execution_frames().collect::<Vec<_>>();
        let substitutions = self
            .active_const_execution_frames()
            .flat_map(|frame| frame.type_substitutions.iter())
            .map(|(name, ty)| (*name, *ty))
            .collect::<SymbolMap<_>>();
        let const_substitutions = frames
            .into_iter()
            .rev()
            .flat_map(|frame| frame.const_substitutions.iter())
            .map(|(name, arg)| (*name, arg.clone()))
            .collect::<SymbolMap<_>>();
        self.substitute_generics_and_consts(ty, &substitutions, &const_substitutions)
    }

    fn resolve_const_const_generic_arg(
        &self,
        mut arg: nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
            && let Some(resolved) = self
                .active_const_execution_frames()
                .find_map(|frame| frame.const_substitutions.get(name))
        {
            arg = resolved.clone();
        }
        arg
    }

    fn resolve_associated_const_projection_for_env(
        &mut self,
        span: Span,
        projection: AssociatedConstProjection,
    ) -> Result<ConstValue, ConstError> {
        let projection = self.substitute_current_const_projection(projection);
        match self.resolve_associated_const_projection(
            projection.self_ty,
            projection.trait_id,
            &projection.trait_args,
            &projection.trait_const_args,
            &projection.name,
        ) {
            Some(nia_trait_solve::AssociatedConstResolution::Const(arg)) => {
                const_value_from_const_generic_arg(&self.resolve_const_const_generic_arg(arg))
                    .ok_or_else(|| ConstError {
                        span,
                        message: format!(
                            "failed to evaluate associated const value `{}`",
                            self.symbol_name(projection.name)
                        ),
                    })
            }
            Some(nia_trait_solve::AssociatedConstResolution::User(user)) => {
                self.eval_user_associated_const_for_env(span, projection.name, *user)
            }
            None => Err(ConstError {
                span,
                message: format!(
                    "failed to resolve associated const value `{}`",
                    self.symbol_name(projection.name)
                ),
            }),
        }
    }

    fn substitute_current_const_projection(
        &mut self,
        mut projection: AssociatedConstProjection,
    ) -> AssociatedConstProjection {
        projection.self_ty = self.substitute_current_const_generics(projection.self_ty);
        projection.trait_args = projection
            .trait_args
            .into_iter()
            .map(|arg| self.substitute_current_const_generics(arg))
            .collect();
        projection.trait_const_args = projection
            .trait_const_args
            .into_iter()
            .map(|arg| self.substitute_current_const_const_arg(arg))
            .collect();
        projection
    }

    fn substitute_current_const_const_arg(
        &mut self,
        mut arg: nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        arg.ty = self.substitute_current_const_generics(arg.ty);
        self.resolve_const_const_generic_arg(arg)
    }

    fn eval_user_associated_const_for_env(
        &mut self,
        span: Span,
        name: SymbolId,
        user: nia_trait_solve::UserAssociatedConst,
    ) -> Result<ConstValue, ConstError> {
        let Some(expr) = self.associated_const_initializer(user.def_id) else {
            let name = self.symbol_name(name);
            return Err(ConstError {
                span,
                message: format!("associated const value `{name}` has no initializer"),
            });
        };
        self.const_call_locals.push(crate::ConstCallFrame {
            module_id: Some(user.impl_module_id),
            function_id: None,
            type_substitutions: user.substitutions,
            const_substitutions: user.const_substitutions,
            ..crate::ConstCallFrame::default()
        });
        let result = nia_const_eval::eval_resolved_const_expr(&expr, self);
        self.const_call_locals.pop();
        result
    }

    fn associated_const_initializer(&self, def_id: GlobalDefId) -> Option<ResolvedConstExpr> {
        if def_id.module_id == self.defs.module_id {
            return self
                .const_module
                .global_initializers()
                .get(&def_id)
                .or_else(|| {
                    self.const_module
                        .deferred_global_initializers()
                        .get(&def_id)
                })
                .cloned();
        }
        let module = (self.program_const_module)(def_id.module_id)?;
        module
            .global_initializers()
            .get(&def_id)
            .or_else(|| module.deferred_global_initializers().get(&def_id))
            .cloned()
    }

    pub(crate) fn local_const_use(&self, expr: &Expr) -> Option<LocalId> {
        let Some(SemanticValueUse::Local(local_id)) =
            self.semantic_uses.node_value_use(&expr.node_key)
        else {
            return None;
        };
        let local = self.locals.locals.get(local_id)?;
        (local.kind == nia_local_resolve::LocalKind::ConstBinding).then_some(local_id)
    }

    fn const_call_local_value(&self, local_id: LocalId) -> Option<ConstValue> {
        self.active_const_execution_frames()
            .find_map(|frame| frame.locals.get(&local_id).cloned())
    }

    fn active_const_execution_frames(&self) -> impl Iterator<Item = &crate::ConstCallFrame> {
        self.const_call_locals
            .iter()
            .rev()
            .scan(true, |inside_execution, frame| {
                if !*inside_execution {
                    return None;
                }
                if frame.module_id.is_some() {
                    *inside_execution = false;
                }
                Some(frame)
            })
    }

    fn bind_const_call_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        is_mutable: bool,
        value: ConstValue,
        ty: Option<nia_const_check::ConstValueType>,
    ) -> Result<(), ConstError> {
        let Some(frame) = self.const_call_locals.last_mut() else {
            return Err(ConstError {
                span,
                message: "internal const function frame is missing".to_string(),
            });
        };
        if is_mutable {
            frame.mutable_locals.insert(local_id);
        }
        frame.locals.insert(local_id, value);
        if let Some(ty) = ty {
            frame.local_types.insert(local_id, ty);
        }
        Ok(())
    }

    fn assign_const_call_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        name: Option<&SymbolId>,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        for frame in self.const_call_locals.iter_mut().rev() {
            if frame.locals.contains_key(&local_id) {
                if !frame.mutable_locals.contains(&local_id) {
                    let name = name
                        .map(|name| self.symbol_name(*name))
                        .unwrap_or_else(|| "receiver".to_string());
                    return Err(ConstError {
                        span,
                        message: format!("cannot assign to immutable const local `{name}`"),
                    });
                }
                frame.locals.insert(local_id, value);
                return Ok(());
            }
            if frame.module_id.is_some() {
                break;
            }
        }
        Err(ConstError {
            span,
            message: name.map_or_else(
                || "unknown const receiver writeback target".to_string(),
                |name| {
                    format!(
                        "unknown const assignment target `{}`",
                        self.symbol_name(*name)
                    )
                },
            ),
        })
    }

    pub(crate) fn global_const_use(&self, expr: &Expr) -> Option<GlobalDefId> {
        if !matches!(
            generic_inst_base(expr).kind,
            ExprKind::Ident(_) | ExprKind::Qualified { .. }
        ) {
            return None;
        }
        let Some(SemanticValueUse::Global(global_id)) =
            self.semantic_uses.node_value_use(&expr.node_key)
        else {
            return None;
        };
        (self.global_def_kind(global_id) == Some(DefKind::Const)).then_some(global_id)
    }

    pub(crate) fn global_def_kind(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.defs_for_module(global_id.module_id)
            .and_then(|defs| defs.as_ref().defs.get(global_id.def_id).map(|def| def.kind))
    }

    fn resolved_const_function(&self, callee: &ResolvedConstExpr) -> Option<GlobalDefId> {
        if let Some(ConstNameResolution::Global(global_id)) = callee.name_resolution()
            && self.global_def_kind(global_id) == Some(DefKind::Function)
        {
            return Some(global_id);
        }
        None
    }

    fn const_function_body(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_const_ir::ResolvedConstFunction> {
        if def_id.module_id == self.defs.module_id {
            return self.const_module.functions().get(&def_id).cloned();
        }
        (self.program_const_module)(def_id.module_id)?
            .functions()
            .get(&def_id)
            .cloned()
    }
}

fn array_literal_values(elems: &nia_ast::ArrayElements) -> Vec<&Expr> {
    match elems {
        nia_ast::ArrayElements::List(elems) => elems.iter().collect(),
        nia_ast::ArrayElements::Repeat { value, .. } => vec![value],
    }
}

fn array_literal_elem_requires_expected(elem: &Expr) -> bool {
    match &elem.kind {
        ExprKind::Null
        | ExprKind::ErrorOk { .. }
        | ExprKind::ErrorErr { .. }
        | ExprKind::Closure { .. } => true,
        ExprKind::ArrayLiteral {
            elems: nia_ast::ArrayElements::List(elems),
        } => elems.is_empty(),
        _ => false,
    }
}

fn explicit_array_literal_len(
    checker: &mut BodyChecker<'_>,
    span: Span,
    elems: &nia_ast::ArrayElements,
) -> Result<Option<u64>, ConstError> {
    Ok(match elems {
        nia_ast::ArrayElements::List(elems) => {
            Some(u64::try_from(elems.len()).map_err(|_| ConstError {
                span,
                message: "array literal length exceeds the semantic limit".to_string(),
            })?)
        }
        nia_ast::ArrayElements::Repeat { count, .. } => {
            Some(checker.eval_array_repeat_count(count)?)
        }
    })
}

fn const_value_from_const_generic_arg(
    arg: &nia_ty::ConstGenericArg,
) -> Option<nia_const_check::ConstValue> {
    match arg.value {
        nia_ty::ConstGenericValue::Int(value) => Some(nia_const_check::ConstValue::Int(value)),
        nia_ty::ConstGenericValue::Bool(value) => Some(nia_const_check::ConstValue::Bool(value)),
        nia_ty::ConstGenericValue::Char(value) => Some(nia_const_check::ConstValue::Int(
            nia_ty::IntConst::unsigned(value as u32 as u128),
        )),
        nia_ty::ConstGenericValue::GenericParam(_) | nia_ty::ConstGenericValue::ConstExpr(_) => {
            None
        }
    }
}
