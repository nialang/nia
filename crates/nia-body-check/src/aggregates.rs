// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::{BodyChecker, generic_inst_base};
use nia_ast::{Expr, ExprKind};
use nia_const_check::{ConstKey, ConstValueType};
use nia_const_eval::{ConstError, ConstValue};
use nia_const_ir::{ConstNameResolution, ResolvedConstExpr};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_item_signatures::{
    EnumSignature, EnumVariantPayloadSignature, EnumVariantSignature, StructSignature,
    UnionSignature,
};
use nia_local_resolve::LocalKind;
use nia_node_id::NodeMap;
use nia_sema::{NamedField, check_required_field_set};

struct EnumInstance<'a> {
    id: GlobalDefId,
    args: &'a [InternedTyId],
    const_args: &'a [nia_ty::ConstGenericArg],
}
use nia_sema_ir::{AssociatedConstProjection, SemanticUseTable, SemanticValueUse};
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
            _ => {
                let error = self.error();
                for elem in array_literal_values(elems) {
                    self.check_expr_with_expected(elem, Some(error));
                }
                if !self.is_error_recovery_ty(array_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        "array literal type is not an array",
                    ));
                }
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

    /// Checks an array literal's length against the length its type
    /// expects and gives the literal's array type. The literal's own length
    /// is its element count, or its repeat count resolved symbolically
    /// ([`Self::array_repeat_count_len`]). Lengths compare by
    /// [`Self::array_lengths_equivalent`], so in a generic body `[0; N]`
    /// matches `[u8; N]` and `[7; 16]` does not, whatever `N` becomes.
    fn check_array_literal_len(
        &mut self,
        span: Span,
        expected: ArrayLenTy,
        elem_ty: InternedTyId,
        elems: &nia_ast::ArrayElements,
    ) -> InternedTyId {
        let actual = match elems {
            nia_ast::ArrayElements::List(values) => match u64::try_from(values.len()) {
                Ok(len) => Some(ArrayLenTy::ConstValue(len)),
                Err(_) => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        "array literal length exceeds the semantic limit",
                    ));
                    None
                }
            },
            nia_ast::ArrayElements::Repeat { count, .. } => {
                match self.array_repeat_count_len(count) {
                    Ok(len) => {
                        self.record_array_repeat_count(count, len.clone());
                        Some(len)
                    }
                    Err(err) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            err.span,
                            format!(
                                "array repeat count is not a valid constant: {}",
                                err.message
                            ),
                        ));
                        None
                    }
                }
            }
        };
        let len = match (expected, actual) {
            (ArrayLenTy::Infer, Some(actual)) => actual,
            (ArrayLenTy::Infer, None) => ArrayLenTy::ConstValue(0),
            (expected, None) => expected,
            (expected, Some(actual)) => {
                if !self.array_lengths_equivalent(&expected, &actual) {
                    let expected_name = self.array_len_name(&expected);
                    let actual_name = self.array_len_name(&actual);
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!(
                            "array literal length mismatch: expected {expected_name}, got {actual_name}"
                        ),
                    ));
                }
                expected
            }
        };
        self.interner.intern(TyKind::Array { len, elem: elem_ty })
    }

    pub(crate) fn check_struct_literal(
        &mut self,
        span: Span,
        aggregate_ty: InternedTyId,
        fields: &[nia_ast::FieldInit],
    ) -> InternedTyId {
        // Type aliases are transparent at value construction. Resolve the
        // expected type before looking up the nominal declaration so an
        // omitted literal has the same field checking as its canonical type.
        let aggregate_ty = self.normalization.normalize(aggregate_ty);
        let (def_id, args, const_args) = match self.expect_ty_kind(aggregate_ty) {
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => (*def_id, args.clone(), const_args.clone()),
            TyKind::Error => {
                self.check_recovery_fields(fields);
                return self.error();
            }
            _ => {
                self.check_recovery_fields(fields);
                if self.is_error_recovery_ty(aggregate_ty) {
                    return self.error();
                }
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
            let summary = format!("unknown struct field `{name}`");
            let mut diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(field.span, summary)
                .note(format!(
                    "the constructor targets a value of type `{}`",
                    self.ty_name(aggregate_ty)
                ));
            if let Some((note, help)) = available_field_context(self, &signature_fields) {
                diagnostic = diagnostic.note(note).help(help);
            }
            self.diagnostics.push(diagnostic.finish());
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

    pub(crate) fn check_recovery_fields(&mut self, fields: &[nia_ast::FieldInit]) {
        let expected = self.error();
        for field in fields {
            self.check_expr_with_expected(&field.value, Some(expected));
        }
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
            let summary = format!("unknown union field `{name}`");
            let mut diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(field.span, summary)
                .note(format!(
                    "the constructor targets a value of type `{}`",
                    self.ty_name(union_ty)
                ));
            if let Some((note, help)) = available_field_context(self, &signature_fields) {
                diagnostic = diagnostic.note(note).help(help);
            }
            self.diagnostics.push(diagnostic.finish());
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
        // `Enum.Variant` reads as field access on a type; the variant path is
        // `Enum::Variant`, or `.Variant` where the type is known.
        if let Some((def_id, args, const_args)) = self.type_prefix_instance(lhs)
            && self.is_enum_def(def_id)
        {
            let enum_ty = self.interner.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            });
            let enum_name = self.ty_name(enum_ty);
            let variant = self.symbol_name(*name);
            self.diagnostics.push(
                Diagnostic::user_error(
                    codes::TYPE_CHECK,
                    "enum variants are selected with `::`, not `.`",
                )
                .primary(
                    span,
                    format!("`.` reads a field of a value, and `{enum_name}` is a type"),
                )
                .help(format!(
                    "write `{enum_name}::{variant}`, or `.{variant}` where the enum type is expected"
                ))
                .finish(),
            );
            return self.error();
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
            let summary = format!("unknown struct field `{name}`");
            let mut diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(format!("the receiver has type `{}`", self.ty_name(lhs_ty)));
            if let Some((note, help)) = available_field_context(self, &fields) {
                diagnostic = diagnostic.note(note).help(help);
            }
            self.diagnostics.push(diagnostic.finish());
            return self.error();
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions)
    }

    /// The type of field `name` read through `ty`, a struct or union value
    /// or a pointer to one, or `None` when `ty` has no such field.
    ///
    /// It reports nothing, so inference can ask before body checking
    /// explains a failed access.
    pub(crate) fn field_type_for_inference(
        &mut self,
        ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let (def_id, args, const_args) = self.field_base_type(ty)?;
        let fields = if self.is_union_def(def_id) {
            self.resolved_union_signature(def_id)?.signature.fields
        } else {
            self.resolved_struct_signature(def_id)?.signature.fields
        };
        let field = fields.iter().find(|field| &field.name == name)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        Some(self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions))
    }

    pub(crate) fn has_named_field(&mut self, ty: InternedTyId, name: &SymbolId) -> bool {
        let Some((def_id, _, _)) = self.field_base_type(ty) else {
            return false;
        };
        if self.is_union_def(def_id) {
            self.resolved_union_signature(def_id)
                .is_some_and(|resolved| {
                    resolved
                        .signature
                        .fields
                        .iter()
                        .any(|field| &field.name == name)
                })
        } else {
            self.resolved_struct_signature(def_id)
                .is_some_and(|resolved| {
                    resolved
                        .signature
                        .fields
                        .iter()
                        .any(|field| &field.name == name)
                })
        }
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
            let summary = format!("unknown union field `{name}`");
            let mut diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(format!(
                    "the receiver has type `{}`",
                    self.nominal_ty_name(def_id, args)
                ));
            if let Some((note, help)) = available_field_context(self, &fields) {
                diagnostic = diagnostic.note(note).help(help);
            }
            self.diagnostics.push(diagnostic.finish());
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
        if let Some(issue) = self
            .program_signature_scope
            .global(def_id)
            .and_then(|signature| signature.type_issue)
        {
            if let Some(current_global) = self.current_global_def_id {
                self.global_type_issues.insert(current_global, issue);
            }
            let message = match issue {
                nia_item_signatures::ProgramGlobalTypeIssue::CyclicInference => {
                    "cannot infer global type from a cyclic initializer dependency; add an explicit type annotation"
                }
            };
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                message,
            ));
            return Some(self.error());
        }
        let ty = self
            .program_signature_scope
            .value_type(def_id)
            .unwrap_or_else(|| self.error());
        Some(ty)
    }

    pub(crate) fn qualified_program_const_type(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        if let Some(ty) = self.program_signature_scope.value_type(def_id) {
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
        let (enum_id, args, const_args) = self.enum_prefix_instance(lhs)?;
        let (prefix_id, prefix_args, prefix_consts) = self.type_prefix_instance(lhs)?;
        if self
            .constructed_nominal_ty(prefix_id, &prefix_args, &prefix_consts)
            .is_none()
        {
            return Some(self.error());
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
        Some(self.enum_ty(enum_id, &args, &const_args))
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

    pub(crate) fn enum_constructor_variant(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<GlobalDefId> {
        if let Some(enum_id) = self.variant_enum(expr)
            && let Some(variant) = self.qualified_value(expr)
            && variant.module_id == enum_id.module_id
        {
            return Some(variant);
        }
        if let Some((enum_id, def_id)) = self.enum_variant_info(expr) {
            return Some(GlobalDefId {
                module_id: enum_id.module_id,
                def_id,
            });
        }
        let expected = self.normalize_aliases_in_type(expected?);
        let (_, return_type) = self.callable_signature(expected)?;
        let (enum_id, def_id) = self.omitted_enum_variant_info(expr, return_type)?;
        Some(GlobalDefId {
            module_id: enum_id.module_id,
            def_id,
        })
    }

    /// Resolve one constructor signature from its nominal instance. Both the
    /// local constraint solver and ordinary expression checking consume this
    /// contract, so payload substitution and result identity cannot diverge.
    pub(crate) fn enum_constructor_signature(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<(Vec<InternedTyId>, InternedTyId)> {
        let variant_id = self.enum_constructor_variant(expr, expected)?;
        let (enum_id, variant) = self.resolved_enum_variant(variant_id)?;
        let (args, const_args) = self.enum_constructor_instance(expr, enum_id, expected);
        let params = self.enum_variant_payload_types(enum_id, variant, &args, &const_args)?;
        Some((params, self.enum_ty(enum_id, &args, &const_args)))
    }

    pub(crate) fn enum_variant_payload_types(
        &mut self,
        enum_id: GlobalDefId,
        variant: EnumVariantSignature,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<Vec<InternedTyId>> {
        let fields = match variant.payload {
            // Unit variants remain ordinary values, including address taking.
            EnumVariantPayloadSignature::Unit => return None,
            EnumVariantPayloadSignature::Tuple(fields) => fields,
            EnumVariantPayloadSignature::Named(fields) => {
                fields.into_iter().map(|field| field.ty).collect()
            }
        };
        Some(self.substitute_enum_payload_types(enum_id, args, const_args, fields))
    }

    pub(crate) fn check_enum_constructor_ref(
        &mut self,
        expr: &Expr,
        is_readonly: bool,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let (params, return_type) = self.enum_constructor_signature(expr, expected)?;
        if !is_readonly {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                "enum constructor pointers must be formed with `&`, not `&mut`",
            ));
            return Some(self.error());
        }
        let ty = self.interner.intern(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic: false,
            abi: nia_ty::FunctionPointerAbi::Nia,
        });
        self.record_expr_node_type(expr, ty);
        Some(ty)
    }

    pub(crate) fn check_enum_constructor_callable(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        self.callable_signature(expected)?;
        self.check_enum_constructor_ref(expr, true, Some(expected))
    }

    pub(crate) fn check_enum_variant_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
    ) -> Option<InternedTyId> {
        let (enum_id, variant_def) = self.enum_variant_info(callee)?;
        let prefix = match &callee.kind {
            nia_ast::ExprKind::Qualified { lhs, .. } => lhs,
            _ => return None,
        };
        let (prefix_id, prefix_args, prefix_consts) = self.type_prefix_instance(prefix)?;
        if self
            .constructed_nominal_ty(prefix_id, &prefix_args, &prefix_consts)
            .is_none()
        {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        let (_, enum_args, enum_const_args) = self.enum_prefix_instance(prefix)?;
        let variant_id = GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_def,
        };
        let (_, variant) = self.resolved_enum_variant(variant_id)?;
        if let EnumVariantPayloadSignature::Tuple(fields) = &variant.payload {
            self.check_enum_tuple_payload(
                expr.span,
                &variant,
                fields,
                args,
                EnumInstance {
                    id: enum_id,
                    args: &enum_args,
                    const_args: &enum_const_args,
                },
            );
            return Some(self.enum_ty(enum_id, &enum_args, &enum_const_args));
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
        Some(self.enum_ty(enum_id, &enum_args, &enum_const_args))
    }

    pub(crate) fn check_omitted_enum_variant_call(
        &mut self,
        expr: &Expr,
        callee: &Expr,
        args: &[Expr],
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        let ExprKind::OmittedMember { name } = &callee.kind else {
            return None;
        };
        let enum_id = self.enum_global_def_id(expected)?;
        let normalized_expected = self.normalize_aliases_in_type(expected);
        let (enum_args, enum_const_args) = match self.interner.get(normalized_expected).cloned() {
            Some(TyKind::Nominal {
                args, const_args, ..
            }) => (args, const_args),
            _ => (Vec::new(), Vec::new()),
        };
        let variant_def = self
            .enum_variant_scope(enum_id)?
            .into_iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| def_id);
        let Some(variant_def) = variant_def else {
            // The enum context still applies when the member is misspelled.
            // Let member checking own that error, and check arguments for
            // independent failures without treating the member as a callee.
            self.check_expr_with_expected(callee, Some(expected));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
        let variant_id = GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_def,
        };
        let (_, variant) = self.resolved_enum_variant(variant_id)?;
        match &variant.payload {
            EnumVariantPayloadSignature::Tuple(fields) => {
                self.check_enum_tuple_payload(
                    expr.span,
                    &variant,
                    fields,
                    args,
                    EnumInstance {
                        id: enum_id,
                        args: &enum_args,
                        const_args: &enum_const_args,
                    },
                );
            }
            EnumVariantPayloadSignature::Unit => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!(
                        "enum variant `{}` expects no payload",
                        self.symbol_name(*name)
                    ),
                ));
            }
            EnumVariantPayloadSignature::Named(_) => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!(
                        "enum variant `{}` expects a named payload literal",
                        self.symbol_name(*name)
                    ),
                ));
            }
        }
        Some(expected)
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
        let Some(ty) = self.constructed_nominal_ty(def_id, &type_args, &const_args) else {
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        };
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
        instance: EnumInstance<'_>,
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
        let (substitutions, const_substitutions) = self.generic_substitutions_and_consts_for_def(
            instance.id,
            instance.args,
            instance.const_args,
        );
        for (index, arg) in args.iter().enumerate() {
            let Some(expected) = fields.get(index).copied() else {
                self.check_expr(arg);
                continue;
            };
            let expected =
                self.substitute_generics_and_consts(expected, &substitutions, &const_substitutions);
            let actual = self.check_expr_with_expected(arg, Some(expected));
            self.expect_expr_type(arg, expected, actual, "enum variant payload");
        }
    }

    pub(crate) fn check_qualified_struct_literal(
        &mut self,
        expr: &Expr,
        target: &Expr,
        fields: &[nia_ast::FieldInit],
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        if let Some((enum_id, variant_def)) = self
            .enum_variant_info(target)
            .or_else(|| expected.and_then(|ty| self.omitted_enum_variant_info(target, ty)))
        {
            if let ExprKind::Qualified { lhs, .. } = &target.kind
                && let Some((prefix_id, args, const_args)) = self.type_prefix_instance(lhs)
                && self
                    .constructed_nominal_ty(prefix_id, &args, &const_args)
                    .is_none()
            {
                self.check_recovery_fields(fields);
                return self.error();
            }
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
                return self.enum_ty(enum_id, &[], &[]);
            };
            let (enum_args, enum_const_args) =
                self.enum_constructor_instance(target, enum_id, expected);
            self.check_named_enum_payload(
                expr.span,
                &variant,
                expected_fields,
                fields,
                EnumInstance {
                    id: enum_id,
                    args: &enum_args,
                    const_args: &enum_const_args,
                },
            );
            return self.enum_ty(enum_id, &enum_args, &enum_const_args);
        }
        if matches!(target.kind, ExprKind::OmittedMember { .. })
            && let Some(expected) = expected
            && self.enum_global_def_id(expected).is_some()
        {
            self.check_expr_with_expected(target, Some(expected));
            self.check_recovery_fields(fields);
            return self.error();
        }
        if let Some((def_id, args, const_args)) = self.type_prefix_instance(target) {
            let Some(ty) = self.constructed_nominal_ty(def_id, &args, &const_args) else {
                self.check_recovery_fields(fields);
                return self.error();
            };
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
        instance: EnumInstance<'_>,
    ) {
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span, field.name)),
            expected_fields.iter().map(|field| field.name),
        );
        let (substitutions, const_substitutions) = self.generic_substitutions_and_consts_for_def(
            instance.id,
            instance.args,
            instance.const_args,
        );
        for field in fields {
            if let Some(expected) = expected_fields
                .iter()
                .find(|expected| expected.name == field.name)
                .map(|field| {
                    self.substitute_generics_and_consts(
                        field.ty,
                        &substitutions,
                        &const_substitutions,
                    )
                })
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

    /// The nominal type a constructor builds. The constructor names its
    /// type, so every generic argument must be written, as in `Pair[i32] {
    /// .. }` or `Option[i32]::Some(1)`; constructors do not infer them
    /// ([`nia_sema::constructor_generic_args`]). The constructor check
    /// reported a wrong count before body checking, so it gives no type here
    /// and no instance whose arguments do not match its parameters reaches
    /// later phases.
    pub(crate) fn constructed_nominal_ty(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<InternedTyId> {
        let declared = self
            .nominal_type_generics(def_id)
            .map_or(0, |generics| generics.len());
        let written = args.len() + const_args.len();
        matches!(
            nia_sema::constructor_generic_args(
                nia_sema::ConstructorSpelling::Prefix { written },
                declared,
            ),
            nia_sema::ConstructorGenericArgs::Written
        )
        .then(|| {
            self.interner.intern(TyKind::Nominal {
                def_id,
                args: args.to_vec(),
                const_args: const_args.to_vec(),
            })
        })
    }

    pub(crate) fn enum_ty(
        &mut self,
        enum_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> InternedTyId {
        self.interner.intern(TyKind::Nominal {
            def_id: enum_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        })
    }

    fn enum_constructor_instance(
        &mut self,
        expr: &Expr,
        enum_id: GlobalDefId,
        expected: Option<InternedTyId>,
    ) -> (Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>) {
        if let ExprKind::Qualified { lhs, .. } = &expr.kind
            && let Some((candidate, args, const_args)) = self.enum_prefix_instance(lhs)
            && candidate == enum_id
        {
            return (args, const_args);
        }
        let Some(expected) = expected else {
            return (Vec::new(), Vec::new());
        };
        let expected = self.normalize_aliases_in_type(expected);
        let expected = match self.interner.get(expected).cloned() {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if def_id == enum_id => return (args, const_args),
            Some(TyKind::FunctionPointer { .. } | TyKind::Callable { .. }) => self
                .callable_signature(expected)
                .map(|(_, return_type)| return_type)
                .unwrap_or(expected),
            _ => expected,
        };
        match self.interner.get(expected).cloned() {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if def_id == enum_id => (args, const_args),
            _ => (Vec::new(), Vec::new()),
        }
    }

    fn substitute_enum_payload_types(
        &mut self,
        enum_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        fields: Vec<InternedTyId>,
    ) -> Vec<InternedTyId> {
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(enum_id, args, const_args);
        fields
            .into_iter()
            .map(|ty| self.substitute_generics_and_consts(ty, &substitutions, &const_substitutions))
            .collect()
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
        let ExprKind::Qualified { lhs, name, .. } = &expr.kind else {
            return None;
        };
        let (enum_id, _, _) = self.enum_prefix_instance(lhs)?;
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some((enum_id, variant_id))
    }

    /// Alias prefixes select the same enum declaration and concrete instance
    /// as their expanded type. Keep this shared by values, constructors, and
    /// patterns; source-prefix arity is still validated on the original alias.
    fn enum_prefix_instance(
        &mut self,
        expr: &Expr,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let (def_id, args, const_args) = self.type_prefix_instance(expr)?;
        let ty = self.interner.intern(TyKind::Nominal {
            def_id,
            args,
            const_args,
        });
        let ty = self.normalize_aliases_in_type(ty);
        let TyKind::Nominal {
            def_id,
            args,
            const_args,
        } = self.interner.get(ty)?.clone()
        else {
            return None;
        };
        self.is_enum_def(def_id)
            .then_some((def_id, args, const_args))
    }

    pub(crate) fn omitted_enum_variant_info(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
    ) -> Option<(GlobalDefId, DefId)> {
        let ExprKind::OmittedMember { name } = &expr.kind else {
            return None;
        };
        let enum_id = self.enum_global_def_id(expected)?;
        let variant_id = self
            .enum_variant_scope(enum_id)?
            .into_iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| def_id)?;
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

impl<'a> BodyChecker<'a> {
    pub(crate) fn lower_const_expr(
        &self,
        expr: &Expr,
    ) -> Result<nia_const_ir::ResolvedConstExpr, nia_const_ir::ConstLowerError> {
        let semantic_uses = self
            .const_semantic_uses
            .get_or_init(|| self.build_const_semantic_uses());
        let context =
            nia_const_ir::ResolvedConstLowerInputs::new(semantic_uses).with_symbols(self.symbols);
        nia_const_ir::lower_expr_resolved_with_context(expr, &context)
    }

    fn build_const_semantic_uses(&self) -> SemanticUseTable {
        let node_value_uses = self
            .semantic_uses
            .node_value_uses
            .filter_map_values(|value_use| match value_use {
                SemanticValueUse::Local(local_id)
                    if self
                        .locals
                        .locals
                        .get(*local_id)
                        .is_some_and(|local| local.kind == LocalKind::ConstBinding) =>
                {
                    Some(*value_use)
                }
                SemanticValueUse::Global(_) => Some(*value_use),
                SemanticValueUse::Local(_) => None,
            });
        let node_store = self.semantic_uses.node_store();
        SemanticUseTable {
            node_value_uses,
            node_const_generic_uses: self.semantic_uses.node_const_generic_uses.clone(),
            node_builtin_associated_values: NodeMap::with_store(node_store),
            node_associated_const_projections: NodeMap::with_store(node_store),
            node_local_defs: self.semantic_uses.node_local_defs.clone(),
            node_type_uses: self.semantic_uses.node_type_uses.clone(),
            node_type_prefixes: self.semantic_uses.node_type_prefixes.clone(),
        }
    }

    /// The length a repeat count denotes, by the classification type
    /// lowering gives an array type's length ([`nia_type_lower::classify_array_len_expr`]):
    /// a const parameter in scope, a layout query of a type, or a value that
    /// constant evaluation computes. A count that depends on a generic
    /// parameter stays symbolic; each instance folds it to a value.
    pub(crate) fn array_repeat_count_len(
        &mut self,
        count: &Expr,
    ) -> Result<ArrayLenTy, ConstError> {
        let expected = self.primitive(nia_ty::PrimitiveTy::Usize);
        let actual =
            self.with_const_context(|this| this.check_expr_with_expected(count, Some(expected)));
        if !self.types_match(expected, actual) {
            return Err(ConstError {
                span: count.span,
                message: "array repeat count must have type `usize`".to_string(),
            });
        }
        if let Some(arg) = self.symbolic_const_expr(count, Some(expected))
            && let Some(len) = nia_ty::array_len_from_const_arg(&arg)
        {
            return Ok(len);
        }
        let count = self.lower_const_expr(count).map_err(|err| ConstError {
            span: err.span,
            message: err.message,
        })?;
        self.evaluate_const_array_len_expr(&count)
            .map(ArrayLenTy::ConstValue)
    }

    pub(crate) fn symbolic_const_expr(
        &mut self,
        expr: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<nia_ty::ConstGenericArg> {
        let expr = self.lower_const_expr(expr).ok()?;
        let result = self.with_typed_const_query(|input| {
            nia_const_check::classify_symbolic_const_expr(input, &expr, expected)
        });
        match result {
            Ok(value) => value,
            Err(error) => {
                self.record_internal(error);
                None
            }
        }
    }

    pub(crate) fn with_typed_const_query<T>(
        &self,
        query: impl FnOnce(nia_const_check::TypedConstQueryInput<'_>) -> T,
    ) -> T {
        let trait_impls_for_module = |_| Some(self.program_trait_impls.to_vec());
        let program_signature_scope = self.program_signature_scope;
        let program_fieldless_enum = |def_id| program_signature_scope.has_fieldless_enum(def_id);
        let function_signature = |def_id: GlobalDefId| {
            if def_id.module_id == self.defs.module_id
                && let Some(signature) = self.const_signatures.functions.get(&def_id.def_id)
            {
                return Some(signature.clone());
            }
            program_signature_scope
                .function(def_id)
                .map(|function| function.signature)
        };
        query(nia_const_check::TypedConstQueryInput {
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
            source_text: self.source_text,
            program: nia_const_check::ConstProgramContext {
                module_path: None,
                module: Some(self.program_const_module),
                source_path: self.program.module_source_path,
                source_text: self.program.module_source_text,
                defs: self.program.defs,
                type_normalizations: self.program.type_normalizations,
                signatures: self.program.signatures,
                function_signature: Some(&function_signature),
                value_signatures: self.program.signatures,
                const_values: Some(self.program_const_values),
                global_initializer: None,
                program_fieldless_enum: Some(&program_fieldless_enum),
                trait_impls_for_module: Some(&trait_impls_for_module),
                visible_extensions: self.program.visible_extensions,
            },
            typed_values: self.const_eval.typed_values,
            array_lengths: self.const_eval.array_lengths,
            frames: &[],
        })
    }

    pub(crate) fn computed_array_len_value(
        &self,
        expr: &std::sync::Arc<nia_ty::ConstComputation>,
    ) -> Result<u64, String> {
        let ty = self.interner.primitive(nia_ty::PrimitiveTy::Usize);
        let arg = nia_ty::ConstGenericArg {
            ty,
            value: nia_ty::ConstGenericValue::Computation(expr.clone()),
        };
        let result = self
            .with_typed_const_query(|input| {
                nia_const_check::evaluate_const_generic_arg(input, &arg)
            })
            .map_err(|error| error.to_string())?;
        if let Some(diagnostic) = result.diagnostics.first() {
            return Err(diagnostic.summary.clone());
        }
        let value = result.value.map_err(|error| error.message)?;
        match value {
            nia_const_check::ConstValue::Int(value) => {
                nia_const_eval::int_to_array_len(expr.span, value).map_err(|error| error.message)
            }
            _ => Err("array length computation must have type usize".into()),
        }
    }

    pub(crate) fn evaluate_const_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Result<ConstValue, ConstError> {
        let evaluation = self.with_typed_const_query(|input| {
            nia_const_check::evaluate_resolved_const_expr(input, expr)
        });
        match evaluation {
            Ok(evaluation) => {
                self.diagnostics.extend(evaluation.diagnostics);
                for demand in evaluation.provider_demands {
                    self.record_provider_demand(demand);
                }
                evaluation.value
            }
            Err(error) => {
                self.record_internal(error);
                Err(ConstError {
                    span: expr.span(),
                    message: "const evaluation failed after an internal compiler error".to_string(),
                })
            }
        }
    }

    pub(crate) fn evaluate_const_int_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Result<nia_ty::IntConst, ConstError> {
        match self.evaluate_const_expr(expr)? {
            ConstValue::Int(value) => Ok(value),
            _ => Err(ConstError {
                span: expr.span(),
                message: "const expression must be an integer".to_string(),
            }),
        }
    }

    pub(crate) fn evaluate_const_array_len_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Result<u64, ConstError> {
        let value = self.evaluate_const_int_expr(expr)?;
        nia_const_eval::int_to_array_len(expr.span(), value)
    }

    pub(crate) fn const_expr_type_for_ir_with_expected(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> Option<nia_const_check::ConstValueType> {
        match self.with_typed_const_query(|input| {
            nia_const_check::infer_resolved_const_expr_type(input, expr, expected)
        }) {
            Ok(ty) => ty,
            Err(error) => {
                self.record_internal(error);
                None
            }
        }
    }

    pub(crate) fn resolve_associated_const_projection_for_env(
        &mut self,
        span: Span,
        projection: AssociatedConstProjection,
    ) -> Result<ConstValue, ConstError> {
        self.evaluate_const_expr(&ResolvedConstExpr::name(
            span,
            ConstNameResolution::AssociatedConstProjection(projection),
        ))
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

    pub(crate) fn global_const_use(&self, expr: &Expr) -> Option<GlobalDefId> {
        let base = generic_inst_base(expr);
        if !matches!(base.kind, ExprKind::Ident(_) | ExprKind::Qualified { .. }) {
            return None;
        }
        // Value resolution records same-named module constants even when a
        // const generic parameter shadows them in this lexical scope. The
        // generic-use fact is the authoritative interpretation for this
        // syntax node; don't fold it as a closed module constant in BIR.
        if self
            .semantic_uses
            .node_const_generic_use(&base.node_key)
            .is_some()
        {
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
}

fn available_field_context(
    checker: &BodyChecker<'_>,
    fields: &[nia_item_signatures::FieldSignature],
) -> Option<(String, String)> {
    if fields.is_empty() {
        return None;
    }
    const MAX_FIELDS: usize = 8;
    let shown = fields
        .iter()
        .take(MAX_FIELDS)
        .map(|field| format!("`{}`", checker.symbol_name(field.name)))
        .collect::<Vec<_>>();
    let omitted = fields.len().saturating_sub(shown.len());
    let note = if omitted == 0 {
        format!("available fields: {}", shown.join(", "))
    } else {
        format!(
            "available fields: {}; {omitted} more omitted",
            shown.join(", ")
        )
    };
    Some((
        note,
        "check the field spelling or use one of the available fields".to_string(),
    ))
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

/// The length an array literal writes: its element count, or its repeat
/// count resolved symbolically.
pub(crate) fn explicit_array_literal_len(
    checker: &mut BodyChecker<'_>,
    span: Span,
    elems: &nia_ast::ArrayElements,
) -> Result<ArrayLenTy, ConstError> {
    match elems {
        nia_ast::ArrayElements::List(elems) => u64::try_from(elems.len())
            .map(ArrayLenTy::ConstValue)
            .map_err(|_| ConstError {
                span,
                message: "array literal length exceeds the semantic limit".to_string(),
            }),
        nia_ast::ArrayElements::Repeat { count, .. } => checker.array_repeat_count_len(count),
    }
}
