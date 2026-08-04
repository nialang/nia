// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BodyChecker, generic_inst_base};
use nia_ast::{
    ArrayElements, AssignOp, BindingStmt, Block, Expr, ExprKind, IndexArg, SliceRange, Stmt,
    StmtKind, SwitchArmBody, UnaryOp,
};
use nia_body_ir::{
    AtomicOrder, AtomicRmwOp, BuiltinConst, BuiltinOperator, BuiltinPlaceMethod, LocalName,
    MemoryIntrinsicOp, PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBinding,
    TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedFieldInit, TypedForIn, TypedIfPattern,
    TypedLocal, TypedLocalKind, TypedLoop, TypedMemoryIntrinsic, TypedMemoryIntrinsicSource,
    TypedPattern, TypedPatternKind, TypedPlace, TypedRange, TypedSliceRange, TypedStmt,
    TypedStmtKind, TypedSwitch, TypedSwitchArm, TypedSwitchArmBody, TypedWhile,
};
use nia_ids::{BuiltinFunction, BuiltinTraitMethod, InternedTyId, ReceiverKind, TraitId};
use nia_item_signatures::FunctionAttribute;
use nia_local_resolve::{LocalBindingName, LocalKind, LocalUse};
use nia_sema_ir::{BracketSuffixResolution, BuiltinOperatorOp, BuiltinValue, ResolvedCall};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, known};
use nia_trait_solve::TraitResolution;
use nia_ty::{ArrayLenTy, BuiltinTrait, PrimitiveTy, TyKind};
use nia_value_resolve::ValueNameResolution;

use crate::literals::{
    decode_byte_string_literal, decode_char_literal, decode_string_literal,
    float_literal_suffix_ty, integer_literal_suffix_ty, numeric_literal_body,
};

mod asm;

impl<'a> BodyChecker<'a> {
    fn local_name(&self, name: LocalBindingName) -> LocalName {
        match name {
            LocalBindingName::Named(name) => LocalName::named(name),
            LocalBindingName::SelfValue => LocalName::SelfValue,
        }
    }

    fn is_array_ty(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Array { .. })
        )
    }

    fn lower_array_literal_with_ty(
        &mut self,
        array: TypedExpr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        if forced_ty == Some(array.ty) {
            return array;
        }
        if let Some(forced_ty) = forced_ty
            && self.is_array_ty(forced_ty)
        {
            if let Some(ty) = self.materialize_inferred_array_type(forced_ty, array.ty) {
                return TypedExpr { ty, ..array };
            }
            return array;
        }
        array
    }

    fn materialize_forced_expr_ty(
        &mut self,
        expr: &Expr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> Option<nia_ids::InternedTyId> {
        let forced_ty = forced_ty?;
        let Some(actual) = self.expr_ty(expr) else {
            return Some(forced_ty);
        };
        self.materialize_inferred_array_type(forced_ty, actual)
            .or(Some(forced_ty))
    }

    fn lower_trait_object_coercion_source_expr(
        &mut self,
        expr: &Expr,
        source_ty: nia_ids::InternedTyId,
    ) -> TypedExpr {
        let source_ty = self.normalization.normalize(source_ty);
        if let Some(TyKind::Pointer { is_readonly, elem }) = self.interner.get(source_ty).cloned()
            && self
                .expr_ty(expr)
                .is_some_and(|actual| self.types_match(elem, actual))
        {
            return TypedExpr {
                span: expr.span,
                ty: source_ty,
                kind: TypedExprKind::Unary {
                    op: if is_readonly {
                        UnaryOp::RefReadOnly
                    } else {
                        UnaryOp::Ref
                    },
                    expr: Box::new(self.lower_expr_with_ty(expr, Some(elem))),
                },
            };
        }
        self.lower_expr_with_ty(expr, Some(source_ty))
    }

    fn numeric_literal_suffix_type(&mut self, expr: &Expr) -> Option<nia_ids::InternedTyId> {
        integer_literal_suffix_ty(expr)
            .map(|primitive| self.primitive(primitive))
            .or_else(|| float_literal_suffix_ty(expr).map(|primitive| self.primitive(primitive)))
    }

    pub(crate) fn lower_body(&mut self, block: &Block) -> TypedBody {
        self.lower_body_with_expected_tail(block, None)
    }

    fn lower_body_with_expected_tail(
        &mut self,
        block: &Block,
        expected_tail: Option<nia_ids::InternedTyId>,
    ) -> TypedBody {
        let stmts = block
            .stmts
            .iter()
            .filter(|stmt| {
                !matches!(&stmt.kind, StmtKind::Using(_))
                    && !matches!(&stmt.kind, StmtKind::Binding(binding) if binding.is_const())
                    && !matches!(&stmt.kind, StmtKind::Static(_))
            })
            .map(|stmt| self.lower_stmt(stmt))
            .collect();
        let tail = block
            .tail
            .as_ref()
            .map(|tail| Box::new(self.lower_expr_with_ty(tail, expected_tail)));
        let ty = tail
            .as_ref()
            .map(|tail| tail.ty)
            .or_else(|| self.block_terminating_never_ty(block))
            .unwrap_or_else(|| self.void());
        TypedBody {
            span: block.span,
            locals: self.lower_locals(block.span),
            stmts,
            tail,
            ty,
        }
    }

    fn block_terminating_never_ty(&mut self, block: &Block) -> Option<nia_ids::InternedTyId> {
        let stmt = block.stmts.last()?;
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => Some(self.never()),
            StmtKind::Expr(expr) if self.expr_ty(expr).is_some_and(|ty| self.is_never(ty)) => {
                Some(self.never())
            }
            _ => None,
        }
    }

    fn lower_locals(&self, body_span: Span) -> Vec<TypedLocal> {
        // Runtime body IR intentionally excludes const bindings: their
        // values are available to later phases through evaluated constants, not
        // through storage-bearing locals.
        self.locals
            .locals
            .iter()
            .filter(|(id, local)| {
                self.current_param_locals.contains(id)
                    || (body_span.start <= local.span.start && local.span.end <= body_span.end)
            })
            .filter_map(|(id, local)| {
                let kind = match local.kind {
                    LocalKind::Param => TypedLocalKind::Param,
                    LocalKind::MutableBinding => TypedLocalKind::MutableBinding,
                    LocalKind::ImmutableBinding => TypedLocalKind::ImmutableBinding,
                    LocalKind::ConstBinding => return None,
                };
                Some(TypedLocal {
                    id,
                    name: self.local_name(local.name),
                    kind,
                    ty: self
                        .local_types
                        .get(&id)
                        .copied()
                        .unwrap_or_else(|| self.error()),
                    span: local.span,
                })
            })
            .collect()
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> TypedStmt {
        let kind = match &stmt.kind {
            StmtKind::Using(_) => TypedStmtKind::Expr(TypedExpr {
                span: stmt.span,
                ty: self.error(),
                kind: TypedExprKind::Error,
            }),
            StmtKind::Static(_) => TypedStmtKind::Expr(TypedExpr {
                span: stmt.span,
                ty: self.error(),
                kind: TypedExprKind::Error,
            }),
            StmtKind::Binding(binding) => self
                .lower_binding_stmt(stmt, binding)
                .map(TypedStmtKind::Binding)
                .unwrap_or_else(|| TypedStmtKind::Expr(self.error_expr(stmt.span))),
            StmtKind::Expr(expr) => TypedStmtKind::Expr(self.lower_expr(expr)),
            StmtKind::Return(value) => TypedStmtKind::Return(
                value
                    .as_ref()
                    .map(|value| self.lower_expr_with_ty(value, Some(self.current_return))),
            ),
            StmtKind::Break => TypedStmtKind::Break,
            StmtKind::Continue => TypedStmtKind::Continue,
            StmtKind::Defer(expr) => TypedStmtKind::Defer(self.lower_expr(expr)),
            StmtKind::ForIn(for_stmt) => {
                let iterable_self_ty = self.expr_ty(&for_stmt.iter).unwrap_or_else(|| self.error());
                let (item_ty, iterator_ty) = self.lower_for_iterable_parts(iterable_self_ty);
                TypedStmtKind::ForIn(Box::new(TypedForIn {
                    pattern: self.lower_pattern(&for_stmt.pattern, item_ty),
                    item_ty,
                    bool_ty: self.bool(),
                    iterable_self_ty,
                    iterator_ty,
                    iter: self.lower_expr(&for_stmt.iter),
                    body: self.lower_body(&for_stmt.body),
                }))
            }
            StmtKind::While(while_stmt) => TypedStmtKind::While(Box::new(TypedWhile {
                cond: self.lower_expr(&while_stmt.cond),
                body: self.lower_body(&while_stmt.body),
            })),
            StmtKind::Loop(loop_stmt) => TypedStmtKind::Loop(Box::new(TypedLoop {
                body: self.lower_body(&loop_stmt.body),
            })),
        };
        TypedStmt {
            span: stmt.span,
            kind,
        }
    }

    fn lower_binding_stmt(&mut self, _stmt: &Stmt, binding: &BindingStmt) -> Option<TypedBinding> {
        let (name, node_key) = self.single_pattern_binding(&binding.pattern)?;
        let local_id = self.local_def(node_key)?;
        let ty = if let Some(ty) = self.local_types.get(&local_id).copied() {
            ty
        } else if let Some(ty) = binding.ty.as_ref() {
            self.ty_for_type(ty)
        } else {
            binding
                .value
                .as_ref()
                .and_then(|value| self.expr_ty(value))
                .unwrap_or_else(|| self.error())
        };
        Some(TypedBinding {
            local_id,
            name: LocalName::named(*name),
            ty,
            value: binding.value.as_ref().map(|value| {
                self.lower_binding_initializer_for_pattern(&binding.pattern, value, ty)
            }),
            is_mutable: binding.is_mutable(),
        })
    }

    fn lower_binding_initializer_for_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        value: &nia_ast::Expr,
        binding_ty: InternedTyId,
    ) -> TypedExpr {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(_) | nia_ast::PatternKind::MutPointer(_) => {
                let input_ty = self.pattern_input_ty(pattern, binding_ty);
                let value = self.lower_expr_with_ty(value, Some(input_ty));
                self.lower_binding_pointer_pattern_initializer(pattern, value)
            }
            _ => self.lower_expr_with_ty(value, Some(binding_ty)),
        }
    }

    fn lower_binding_pointer_pattern_initializer(
        &mut self,
        pattern: &nia_ast::Pattern,
        value: TypedExpr,
    ) -> TypedExpr {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(value.ty)) {
                    Some(TyKind::Pointer { elem, .. }) => *elem,
                    _ => self.error(),
                };
                let deref = TypedExpr {
                    span: value.span,
                    ty: elem_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Deref,
                        expr: Box::new(value),
                    },
                };
                self.lower_binding_pointer_pattern_initializer(inner, deref)
            }
            _ => value,
        }
    }

    fn single_pattern_binding<'b>(
        &self,
        pattern: &'b nia_ast::Pattern,
    ) -> Option<(&'b SymbolId, &'b nia_node_id::VersionedNodeKey)> {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { name, node_key, .. } => Some((name, node_key)),
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                self.single_pattern_binding(inner)
            }
            _ => None,
        }
    }

    fn error_expr(&mut self, span: Span) -> TypedExpr {
        TypedExpr {
            span,
            ty: self.error(),
            kind: TypedExprKind::Error,
        }
    }

    fn lower_switch(&mut self, switch: &nia_ast::SwitchStmt) -> TypedSwitch {
        let target = self.lower_expr(&switch.target);
        let target_ty = target.ty;
        TypedSwitch {
            target,
            bool_ty: self.bool(),
            arms: switch
                .arms
                .iter()
                .map(|arm| TypedSwitchArm {
                    patterns: arm
                        .patterns
                        .iter()
                        .map(|pattern| self.lower_pattern(pattern, target_ty))
                        .collect(),
                    body: match &arm.body {
                        SwitchArmBody::Expr(expr) => {
                            TypedSwitchArmBody::Expr(self.lower_expr(expr))
                        }
                        SwitchArmBody::Stmt(stmt) => {
                            TypedSwitchArmBody::Stmt(Box::new(self.lower_stmt(stmt)))
                        }
                        SwitchArmBody::Block(block) => {
                            TypedSwitchArmBody::Block(Box::new(self.lower_body(block)))
                        }
                    },
                    span: arm.span,
                })
                .collect(),
        }
    }

    fn lower_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        target_ty: nia_ids::InternedTyId,
    ) -> TypedPattern {
        let kind = match &pattern.kind {
            nia_ast::PatternKind::Wildcard => TypedPatternKind::Wildcard,
            nia_ast::PatternKind::Bind { name, node_key, .. } => {
                let local_id = self
                    .local_def(node_key)
                    .unwrap_or(nia_ids::LocalId(u32::MAX));
                TypedPatternKind::Bind {
                    local_id,
                    name: LocalName::named(*name),
                }
            }
            nia_ast::PatternKind::Pointer(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Pointer { elem, .. }) => *elem,
                    _ => self.error(),
                };
                TypedPatternKind::Pointer(Box::new(self.lower_pattern(inner, elem_ty)))
            }
            nia_ast::PatternKind::MutPointer(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Pointer { elem, .. }) => *elem,
                    _ => self.error(),
                };
                TypedPatternKind::MutPointer(Box::new(self.lower_pattern(inner, elem_ty)))
            }
            nia_ast::PatternKind::OptionalSome(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Optional { elem }) => *elem,
                    _ => self.error(),
                };
                TypedPatternKind::OptionalSome(Box::new(self.lower_pattern(inner, elem_ty)))
            }
            nia_ast::PatternKind::OptionalNull => TypedPatternKind::OptionalNull,
            nia_ast::PatternKind::ErrorOk(inner) => {
                let value_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { value, .. }) => *value,
                    _ => self.error(),
                };
                TypedPatternKind::ErrorOk(Box::new(self.lower_pattern(inner, value_ty)))
            }
            nia_ast::PatternKind::ErrorErr(inner) => {
                let error_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { error, .. }) => *error,
                    _ => self.error(),
                };
                TypedPatternKind::ErrorErr(Box::new(self.lower_pattern(inner, error_ty)))
            }
            nia_ast::PatternKind::EnumVariant { variant, fields } => {
                let variant_id =
                    self.enum_variant_info(variant)
                        .map(|(enum_id, def_id)| nia_ids::GlobalDefId {
                            module_id: enum_id.module_id,
                            def_id,
                        });
                let typed_fields = variant_id
                    .and_then(|variant_id| {
                        self.resolved_enum_variant(variant_id)
                            .and_then(|(enum_id, signature)| {
                                let backing_type = self
                                    .resolved_enum_signature(enum_id)?
                                    .signature
                                    .backing_type;
                                Some((variant_id, backing_type, signature))
                            })
                    })
                    .map(|(variant_id, backing_type, signature)| {
                        let fields = match (signature.payload, fields) {
                            (
                                nia_item_signatures::EnumVariantPayloadSignature::Tuple(types),
                                nia_ast::EnumVariantPatternFields::Tuple(patterns),
                            ) => patterns
                                .iter()
                                .zip(types)
                                .map(|(pattern, ty)| self.lower_pattern(pattern, ty))
                                .collect(),
                            (
                                nia_item_signatures::EnumVariantPayloadSignature::Named(expected),
                                nia_ast::EnumVariantPatternFields::Named(patterns),
                            ) => expected
                                .into_iter()
                                .filter_map(|expected| {
                                    let pattern = patterns
                                        .iter()
                                        .find(|pattern| pattern.name == expected.name)?;
                                    Some(self.lower_pattern(&pattern.pattern, expected.ty))
                                })
                                .collect(),
                            _ => Vec::new(),
                        };
                        (variant_id, backing_type, fields)
                    });
                if let Some((variant, backing_type, fields)) = typed_fields {
                    TypedPatternKind::EnumVariant {
                        variant,
                        backing_type,
                        fields,
                    }
                } else {
                    TypedPatternKind::Wildcard
                }
            }
            nia_ast::PatternKind::Expr(expr) => self.lower_pattern_expr(expr, target_ty),
            nia_ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => self.lower_pattern_range(start, end, *inclusive, target_ty),
        };
        TypedPattern {
            ty: target_ty,
            span: pattern.span,
            kind,
        }
    }

    fn lower_pattern_expr(
        &mut self,
        expr: &Expr,
        target_ty: nia_ids::InternedTyId,
    ) -> TypedPatternKind {
        if let Some(variant) = self.qualified_enum_variant(expr)
            && let Some((enum_id, _)) = self.resolved_enum_variant(variant)
            && let Some(signature) = self.resolved_enum_signature(enum_id)
        {
            return TypedPatternKind::EnumVariant {
                variant,
                backing_type: signature.signature.backing_type,
                fields: Vec::new(),
            };
        }
        if (self.is_integer(target_ty) || self.is_bool(target_ty))
            && let Some(value) = self.node_pattern_values.get(&expr.node_key).copied()
        {
            return TypedPatternKind::CheckedInt { value };
        }
        TypedPatternKind::Expr(self.lower_expr(expr))
    }

    fn lower_pattern_range(
        &mut self,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        target_ty: nia_ids::InternedTyId,
    ) -> TypedPatternKind {
        if self.is_integer(target_ty) {
            let start_value = self.node_pattern_values.get(&start.node_key).copied();
            let end_value = self.node_pattern_values.get(&end.node_key).copied();
            if let (Some(start), Some(end)) = (start_value, end_value) {
                return TypedPatternKind::CheckedIntRange {
                    start,
                    end,
                    inclusive,
                };
            }
        }
        TypedPatternKind::Range {
            start: Box::new(self.lower_expr(start)),
            end: Box::new(self.lower_expr(end)),
            inclusive,
        }
    }

    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> TypedExpr {
        self.lower_expr_with_ty(expr, None)
    }

    fn lower_expr_with_ty(
        &mut self,
        expr: &Expr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        if let Some(upcast) = self
            .node_trait_object_upcasts
            .get(&expr.node_key)
            .copied()
            .filter(|upcast| forced_ty.is_none_or(|forced_ty| forced_ty == upcast.target_ty))
        {
            return TypedExpr {
                span: expr.span,
                ty: upcast.target_ty,
                kind: TypedExprKind::TraitObjectUpcast {
                    expr: Box::new(self.lower_expr_with_ty(expr, Some(upcast.source_ty))),
                    source_ty: upcast.source_ty,
                    target_ty: upcast.target_ty,
                },
            };
        }
        if let Some(coercion) = self
            .node_trait_object_coercions
            .get(&expr.node_key)
            .copied()
            .filter(|coercion| forced_ty.is_none_or(|forced_ty| forced_ty == coercion.target_ty))
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.target_ty,
                kind: TypedExprKind::TraitObjectCoercion {
                    expr: Box::new(
                        self.lower_trait_object_coercion_source_expr(expr, coercion.source_ty),
                    ),
                    target_ty: coercion.target_ty,
                    self_ty: coercion.self_ty,
                },
            };
        }
        if let Some(coercion) = self
            .node_pointer_array_to_slice_coercions
            .get(&expr.node_key)
            .copied()
            && forced_ty.is_none_or(|forced_ty| forced_ty == coercion.slice_ty)
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.slice_ty,
                kind: TypedExprKind::Slice {
                    lhs: Box::new(self.lower_expr_with_ty(expr, Some(coercion.pointer_ty))),
                    range: TypedSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: coercion.is_readonly,
                },
            };
        }
        let forced_ty = self
            .materialize_forced_expr_ty(expr, forced_ty)
            .filter(|ty| !self.is_error_ty(*ty));
        let ty = forced_ty
            .or_else(|| self.expr_ty(expr))
            .unwrap_or_else(|| self.error());
        if let Some(def_id) = self.global_const_use(expr) {
            let ty = forced_ty.unwrap_or_else(|| self.runtime_ty_for_global_const_use(def_id, ty));
            return self.lower_const_value_expr(expr.span, ty, self.global_const_value(def_id));
        }
        if let Some(variant_id) = self.qualified_enum_variant(expr) {
            return TypedExpr {
                span: expr.span,
                ty,
                kind: TypedExprKind::EnumVariant {
                    variant: variant_id,
                    fields: Vec::new(),
                },
            };
        }
        if let Some(def_id) = self.qualified_value(expr) {
            let kind = match self.global_def_kind(def_id) {
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                    TypedExprKind::Function(def_id)
                }
                Some(nia_defs::DefKind::Global) => TypedExprKind::Global(def_id),
                Some(nia_defs::DefKind::Const) => {
                    let ty = forced_ty
                        .unwrap_or_else(|| self.runtime_ty_for_global_const_use(def_id, ty));
                    return self.lower_const_value_expr(
                        expr.span,
                        ty,
                        self.global_const_value(def_id),
                    );
                }
                _ => TypedExprKind::Error,
            };
            return TypedExpr {
                span: expr.span,
                ty,
                kind,
            };
        }
        let mut lowered_ty = ty;
        let kind = match &expr.kind {
            ExprKind::Error | ExprKind::Raw(_) | ExprKind::Underscore | ExprKind::PathRoot(_) => {
                TypedExprKind::Error
            }
            ExprKind::Integer(text) => {
                TypedExprKind::Integer(numeric_literal_body(text).to_string())
            }
            ExprKind::Float(text) => TypedExprKind::Float(numeric_literal_body(text).to_string()),
            ExprKind::String(literal) => {
                let array = TypedExpr {
                    span: expr.span,
                    ty: self.string_literal_array_type(literal),
                    kind: TypedExprKind::String(decode_string_literal(literal).unwrap_or_default()),
                };
                return self.lower_array_literal_with_ty(array, forced_ty);
            }
            ExprKind::ByteString(literal) => {
                let array = TypedExpr {
                    span: expr.span,
                    ty: self.byte_string_literal_array_type(literal),
                    kind: TypedExprKind::ByteString(
                        decode_byte_string_literal(literal).unwrap_or_default(),
                    ),
                };
                return self.lower_array_literal_with_ty(array, forced_ty);
            }
            ExprKind::Char(text) => TypedExprKind::Char(decode_char_literal(text).unwrap_or(0)),
            ExprKind::ByteChar(text) => TypedExprKind::ByteChar(text.clone()),
            ExprKind::Bool(value) => TypedExprKind::Bool(*value),
            ExprKind::Null => TypedExprKind::Null,
            ExprKind::Ident(_) | ExprKind::SelfValue => {
                if let Some(local_id) = self.local_const_use(expr) {
                    return self.lower_const_value_expr(
                        expr.span,
                        ty,
                        self.const_eval
                            .values
                            .get(&nia_const_check::ConstKey::Local(local_id))
                            .cloned(),
                    );
                }
                self.lower_ident_expr(expr)
            }
            ExprKind::Qualified { .. } if self.builtin_value(expr).is_some() => {
                match self.builtin_value(expr) {
                    Some(BuiltinValue::Int(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Int(*value))
                    }
                    Some(BuiltinValue::Usize(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                    Some(BuiltinValue::Layout { builtin, ty }) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Layout {
                            builtin: *builtin,
                            ty: *ty,
                        })
                    }
                    Some(BuiltinValue::FieldOffset { ty, field }) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::FieldOffset {
                            ty: *ty,
                            field: *field,
                        })
                    }
                    None => TypedExprKind::Error,
                }
            }
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => TypedExprKind::Error,
            ExprKind::BracketSuffix { callee, args } => {
                match self.bracket_suffix_resolution(expr) {
                    Some(BracketSuffixResolution::Index) => {
                        if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                            let lhs_expected = if self.expr_ty(callee).is_none() {
                                self.index_lhs_expected_from_index_expected(forced_ty)
                            } else {
                                None
                            };
                            self.lower_index_expr(callee, index).unwrap_or_else(|| {
                                TypedExprKind::Index {
                                    lhs: Box::new(self.lower_expr_with_ty(callee, lhs_expected)),
                                    index: Box::new(self.lower_expr(index)),
                                }
                            })
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::GenericCall) => {
                        if let Some(reference) = self.function_reference(expr) {
                            TypedExprKind::FunctionInstance {
                                def_id: reference.def_id,
                                arg_module_id: reference.arg_module_id,
                                args: reference.args.clone(),
                                const_args: reference.const_args.clone(),
                            }
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::TypePrefixInstantiation) | None => {
                        TypedExprKind::Error
                    }
                }
            }
            ExprKind::Field { lhs, name } => self
                .lower_field_access_expr(lhs, name)
                .unwrap_or(TypedExprKind::Error),
            ExprKind::ArrayLiteral { elems } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems, ty),
            },
            ExprKind::TypedArrayLiteral { elems, .. } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems, ty),
            },
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                let Some(def_id) = self.nominal_global_def(ty) else {
                    return TypedExpr {
                        span: expr.span,
                        ty,
                        kind: TypedExprKind::Error,
                    };
                };
                if self.is_union_def(def_id) {
                    let field = fields.first().map(|field| {
                        let field_def = self.field_def_for_aggregate_ty(ty, &field.name);
                        let field_ty = self.field_ty_for_aggregate_ty(ty, &field.name);
                        TypedFieldInit {
                            field: field_def,
                            name: self.symbol_name(field.name),
                            value: self.lower_expr_with_ty(&field.value, field_ty),
                            span: field.span,
                        }
                    });
                    TypedExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(field.unwrap_or_else(|| TypedFieldInit {
                            field: None,
                            name: String::new(),
                            value: TypedExpr {
                                span: expr.span,
                                ty: self.error(),
                                kind: TypedExprKind::Error,
                            },
                            span: expr.span,
                        })),
                    }
                } else {
                    TypedExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .iter()
                            .map(|field| {
                                let field_def = self.field_def_for_aggregate_ty(ty, &field.name);
                                let field_ty = self.field_ty_for_aggregate_ty(ty, &field.name);
                                TypedFieldInit {
                                    field: field_def,
                                    name: self.symbol_name(field.name),
                                    value: self.lower_expr_with_ty(&field.value, field_ty),
                                    span: field.span,
                                }
                            })
                            .collect(),
                    }
                }
            }
            ExprKind::QualifiedStructLiteral { target, fields } => {
                if let Some((enum_id, variant_def)) = self.enum_variant_info(target) {
                    let variant = nia_ids::GlobalDefId {
                        module_id: enum_id.module_id,
                        def_id: variant_def,
                    };
                    let ordered = self
                        .resolved_enum_variant(variant)
                        .and_then(|(_, signature)| match signature.payload {
                            nia_item_signatures::EnumVariantPayloadSignature::Named(expected) => {
                                Some(expected)
                            }
                            _ => None,
                        })
                        .map(|expected| {
                            expected
                                .into_iter()
                                .filter_map(|expected| {
                                    let field =
                                        fields.iter().find(|field| field.name == expected.name)?;
                                    Some(self.lower_expr_with_ty(&field.value, Some(expected.ty)))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    TypedExprKind::EnumVariant {
                        variant,
                        fields: ordered,
                    }
                } else {
                    let Some(def_id) = self.nominal_global_def(ty) else {
                        return TypedExpr {
                            span: expr.span,
                            ty,
                            kind: TypedExprKind::Error,
                        };
                    };
                    TypedExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .iter()
                            .map(|field| {
                                let field_ty = self.field_ty_for_aggregate_ty(ty, &field.name);
                                TypedFieldInit {
                                    field: self.field_def_for_aggregate_ty(ty, &field.name),
                                    name: self.symbol_name(field.name),
                                    value: self.lower_expr_with_ty(&field.value, field_ty),
                                    span: field.span,
                                }
                            })
                            .collect(),
                    }
                }
            }
            ExprKind::Unary { op, expr: inner }
                if let Some(trait_id) = BuiltinOperatorOp::Unary(*op).trait_id() =>
            {
                let inner_ty = self
                    .numeric_literal_suffix_type(expr)
                    .or_else(|| self.expr_ty(expr))
                    .or_else(|| self.expr_ty(inner))
                    .or(forced_ty);
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Unary(*op),
                    }),
                    args: vec![self.lower_expr_with_ty(inner, inner_ty)],
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                let inner_ty = self.expected_ref_target_from_expected(*op, forced_ty);
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                    && matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                {
                    self.lower_slice_expr(lhs, range, matches!(op, UnaryOp::RefReadOnly))
                } else if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && let Some(function_item) = self.lower_function_item_ref(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(TypedExpr {
                            span: inner.span,
                            ty: self.expr_ty(inner).unwrap_or_else(|| self.error()),
                            kind: function_item,
                        }),
                    }
                } else if matches!(op, UnaryOp::Deref)
                    && let Some(pointer) = self.lower_deref_pointer(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(pointer),
                    }
                } else if matches!(op, UnaryOp::Deref) {
                    let inner_ty = self.pointer_to_deref_expected(forced_ty);
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                    }
                } else if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly) {
                    let inner = self.lower_expr_with_ty(inner, inner_ty);
                    lowered_ty = self.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(op, UnaryOp::RefReadOnly),
                        elem: inner.ty,
                    });
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(inner),
                    }
                } else {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(self.lower_expr(inner)),
                    }
                }
            }
            ExprKind::OptionalSome { expr: inner } => {
                let inner_ty = self.optional_elem_ty(ty).or_else(|| self.expr_ty(inner));
                TypedExprKind::OptionalSome {
                    expr: Box::new(if matches!(inner.kind, ExprKind::Try { .. }) {
                        self.lower_expr_with_checked_ty(inner)
                    } else {
                        self.lower_expr_with_ty(inner, inner_ty)
                    }),
                }
            }
            ExprKind::ErrorOk { expr: inner } => {
                let inner_ty = self
                    .error_union_parts(ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.expr_ty(inner));
                TypedExprKind::ErrorOk {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                }
            }
            ExprKind::ErrorErr { expr: inner } => {
                let inner_ty = self
                    .error_union_parts(ty)
                    .map(|(error, _)| error)
                    .or_else(|| self.expr_ty(inner));
                TypedExprKind::ErrorErr {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                }
            }
            ExprKind::Try { expr: inner } => TypedExprKind::Try {
                expr: Box::new(self.lower_expr_with_checked_ty(inner)),
            },
            ExprKind::Binary { lhs, op, rhs }
                if let Some(trait_id) = BuiltinOperatorOp::Binary(*op).trait_id() =>
            {
                let lhs_expr = self.lower_expr(lhs);
                let rhs_ty = self.expr_ty(rhs).or_else(|| {
                    self.can_expected_type_drive_builtin_operator(lhs_expr.ty, *op)
                        .then_some(lhs_expr.ty)
                });
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Binary(*op),
                    }),
                    args: vec![lhs_expr, self.lower_expr_with_ty(rhs, rhs_ty)],
                }
            }
            ExprKind::Binary { lhs, op, rhs } => TypedExprKind::Binary {
                lhs: Box::new(self.lower_expr(lhs)),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Assign {
                lhs,
                op: AssignOp::Assign,
                rhs,
            } if matches!(lhs.kind, ExprKind::Underscore) => {
                TypedExprKind::Discard(Box::new(self.lower_expr(rhs)))
            }
            ExprKind::Assign { lhs, op, rhs } => TypedExprKind::Assign {
                place: self.lower_place(lhs),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Cast { expr: inner, ty } => {
                let inner_ty = self
                    .numeric_literal_suffix_type(inner)
                    .or_else(|| self.expr_ty(inner));
                TypedExprKind::Cast {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                    ty: self.ty_for_type(ty),
                }
            }
            ExprKind::Call { callee, args } => {
                if let Some((enum_id, variant_def)) = self.enum_variant_info(callee) {
                    TypedExprKind::EnumVariant {
                        variant: nia_ids::GlobalDefId {
                            module_id: enum_id.module_id,
                            def_id: variant_def,
                        },
                        fields: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                    }
                } else if let Some(ResolvedCall::BuiltinFunction { builtin, type_arg }) =
                    self.resolved_call(expr)
                {
                    self.lower_builtin_function_call(expr, builtin, type_arg, args)
                } else if let Some(ResolvedCall::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                }) = self.resolved_call(expr)
                {
                    let receiver_ty = self.receiver_ty_for_target(self_ty, method.receiver_kind());
                    let (receiver, lowered_args) =
                        self.lower_builtin_call_receiver(callee, args, Some(receiver_ty));
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                            trait_id,
                            method,
                            self_ty,
                            trait_args,
                            receiver: Box::new(self.lower_typed_builtin_place_method_receiver(
                                &receiver, self_ty, method,
                            )),
                        }),
                        args: lowered_args,
                    }
                } else if let Some(ResolvedCall::BuiltinMethod { method, self_ty }) =
                    self.resolved_call(expr)
                {
                    let (receiver, lowered_args) =
                        self.lower_builtin_call_receiver(callee, args, Some(self_ty));
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinMethod {
                            method,
                            self_ty,
                            receiver: Box::new(receiver),
                        },
                        args: lowered_args,
                    }
                } else if let Some(ResolvedCall::BuiltinTraitMethod { trait_id, op, .. }) =
                    self.resolved_call(expr)
                {
                    let lowered_args = self.lower_builtin_trait_method_call_args(callee, args);
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op }),
                        args: lowered_args,
                    }
                } else if let Some((builtin, type_arg)) = self.resolved_builtin_attribute_call(expr)
                {
                    self.lower_builtin_function_call(expr, builtin, type_arg, args)
                } else {
                    TypedExprKind::Call {
                        callee: self.lower_callee(expr, callee, args),
                        args: self.lower_call_args(expr, callee, args),
                    }
                }
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(variant) = self
                    .qualified_enum_variant(expr)
                    .or_else(|| self.enum_variant_for_qualified(lhs, name))
                {
                    TypedExprKind::EnumVariant {
                        variant,
                        fields: Vec::new(),
                    }
                } else {
                    self.lower_field_access_expr(lhs, name)
                        .unwrap_or(TypedExprKind::Error)
                }
            }
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Expr(index) => {
                    let lhs_expected = if self.expr_ty(lhs).is_none() {
                        self.index_lhs_expected_from_index_expected(forced_ty)
                    } else {
                        None
                    };
                    self.lower_index_expr(lhs, index)
                        .unwrap_or_else(|| TypedExprKind::Index {
                            lhs: Box::new(self.lower_expr_with_ty(lhs, lhs_expected)),
                            index: Box::new(self.lower_expr(index)),
                        })
                }
                IndexArg::Range(range) => self.lower_slice_expr_readonly(lhs, range),
            },
            ExprKind::Range(range) => TypedExprKind::Range(self.lower_range(range)),
            ExprKind::Block(block) if self.empty_struct_literal_expr(ty, block) => self
                .nominal_global_def(ty)
                .map(|def_id| TypedExprKind::StructLiteral {
                    def_id,
                    fields: Vec::new(),
                })
                .unwrap_or(TypedExprKind::Error),
            ExprKind::Block(block) => TypedExprKind::Block(self.lower_body_with_expected_tail(
                block,
                if self.is_never(ty) { None } else { Some(ty) },
            )),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => TypedExprKind::If {
                cond: Box::new(self.lower_expr(cond)),
                then_branch: self.lower_body_with_expected_tail(
                    then_branch,
                    if self.is_never(ty) { None } else { Some(ty) },
                ),
                else_branch: else_branch.as_ref().map(|else_branch| {
                    Box::new(self.lower_expr_with_ty(
                        else_branch,
                        if self.is_never(ty) { None } else { Some(ty) },
                    ))
                }),
            },
            ExprKind::IfPattern(if_pattern) => {
                let target = self.lower_expr(&if_pattern.target);
                let target_ty = target.ty;
                TypedExprKind::IfPattern(Box::new(TypedIfPattern {
                    target,
                    bool_ty: self.bool(),
                    pattern: self.lower_pattern(&if_pattern.pattern, target_ty),
                    then_branch: self.lower_body_with_expected_tail(
                        &if_pattern.then_branch,
                        if self.is_never(ty) { None } else { Some(ty) },
                    ),
                    else_branch: if_pattern.else_branch.as_ref().map(|else_branch| {
                        Box::new(self.lower_expr_with_ty(
                            else_branch,
                            if self.is_never(ty) { None } else { Some(ty) },
                        ))
                    }),
                }))
            }
            ExprKind::Switch(switch) => TypedExprKind::Switch(Box::new(self.lower_switch(switch))),
        };
        TypedExpr {
            span: expr.span,
            ty: lowered_ty,
            kind,
        }
    }

    fn resolved_builtin_attribute_call(
        &mut self,
        expr: &Expr,
    ) -> Option<(BuiltinFunction, Option<InternedTyId>)> {
        match self.resolved_call(expr)? {
            ResolvedCall::Function(def_id) => self
                .builtin_attribute_for_function(def_id)
                .map(|builtin| (builtin, None)),
            ResolvedCall::FunctionInstance { def_id, args, .. } => self
                .builtin_attribute_for_function(def_id)
                .map(|builtin| (builtin, args.first().copied())),
            _ => None,
        }
    }

    fn builtin_attribute_for_function(
        &mut self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<BuiltinFunction> {
        self.resolved_function_signature(def_id)?
            .signature
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                FunctionAttribute::Builtin(builtin) => Some(*builtin),
                FunctionAttribute::Naked => None,
            })
    }

    fn memory_intrinsic_elem_ty(&mut self, expr: &Expr) -> nia_ids::InternedTyId {
        let ExprKind::Call { args, .. } = &expr.kind else {
            return self.error();
        };
        let Some(dest) = args.first() else {
            return self.error();
        };
        let Some(dest_ty) = self.expr_ty(dest) else {
            return self.error();
        };
        match self.interner.get(self.normalization.normalize(dest_ty)) {
            Some(TyKind::Slice { elem, .. }) | Some(TyKind::Array { elem, .. }) => *elem,
            _ => self.error(),
        }
    }

    fn lower_builtin_function_call(
        &mut self,
        expr: &Expr,
        builtin: nia_ids::BuiltinFunction,
        type_arg: Option<nia_ids::InternedTyId>,
        args: &[Expr],
    ) -> TypedExprKind {
        match (builtin, args) {
            (nia_ids::BuiltinFunction::Trap, []) => TypedExprKind::Trap,
            (nia_ids::BuiltinFunction::SizeOf | nia_ids::BuiltinFunction::AlignOf, []) => {
                self.lower_builtin_value_expr(expr)
            }
            (nia_ids::BuiltinFunction::Offset, [_]) => self.lower_builtin_value_expr(expr),
            (nia_ids::BuiltinFunction::CharFromU32, [value]) => TypedExprKind::CharFromU32 {
                value: Box::new(self.lower_expr(value)),
            },
            (nia_ids::BuiltinFunction::Asm, [arg]) => self.lower_inline_asm(arg),
            (nia_ids::BuiltinFunction::MemCopy, [dest, source]) => {
                TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                    op: MemoryIntrinsicOp::Copy,
                    elem_ty: self.memory_intrinsic_elem_ty(expr),
                    dest: Box::new(self.lower_expr(dest)),
                    source: TypedMemoryIntrinsicSource::Slice(Box::new(self.lower_expr(source))),
                })
            }
            (nia_ids::BuiltinFunction::MemMove, [dest, source]) => {
                TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                    op: MemoryIntrinsicOp::Move,
                    elem_ty: self.memory_intrinsic_elem_ty(expr),
                    dest: Box::new(self.lower_expr(dest)),
                    source: TypedMemoryIntrinsicSource::Slice(Box::new(self.lower_expr(source))),
                })
            }
            (nia_ids::BuiltinFunction::MemSet, [dest, value]) => {
                TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                    op: MemoryIntrinsicOp::Set,
                    elem_ty: self.memory_intrinsic_elem_ty(expr),
                    dest: Box::new(self.lower_expr(dest)),
                    source: TypedMemoryIntrinsicSource::Byte(Box::new(self.lower_expr(value))),
                })
            }
            (nia_ids::BuiltinFunction::LoadUnaligned, [ptr]) => TypedExprKind::LoadUnaligned {
                ty: type_arg.unwrap_or_else(|| self.error()),
                ptr: Box::new(self.lower_expr(ptr)),
            },
            (nia_ids::BuiltinFunction::Splat, [value]) => TypedExprKind::Splat {
                value: Box::new(self.lower_expr(value)),
            },
            (nia_ids::BuiltinFunction::Extract, [vector, index]) => TypedExprKind::ExtractElement {
                vector: Box::new(self.lower_expr(vector)),
                index: Box::new(self.lower_expr(index)),
            },
            (nia_ids::BuiltinFunction::Insert, [vector, index, value]) => {
                TypedExprKind::InsertElement {
                    vector: Box::new(self.lower_expr(vector)),
                    index: Box::new(self.lower_expr(index)),
                    value: Box::new(self.lower_expr(value)),
                }
            }
            (nia_ids::BuiltinFunction::Bitmask, [vector]) => TypedExprKind::Bitmask {
                vector: Box::new(self.lower_expr(vector)),
            },
            (nia_ids::BuiltinFunction::Ctz, [value]) => TypedExprKind::BitIntrinsic {
                op: nia_body_ir::TypedBitIntrinsicOp::Ctz,
                value: Box::new(self.lower_expr(value)),
            },
            (nia_ids::BuiltinFunction::Clz, [value]) => TypedExprKind::BitIntrinsic {
                op: nia_body_ir::TypedBitIntrinsicOp::Clz,
                value: Box::new(self.lower_expr(value)),
            },
            (nia_ids::BuiltinFunction::Popcount, [value]) => TypedExprKind::BitIntrinsic {
                op: nia_body_ir::TypedBitIntrinsicOp::Popcount,
                value: Box::new(self.lower_expr(value)),
            },
            (nia_ids::BuiltinFunction::AtomicLoad, [ptr, order]) => {
                TypedExprKind::Atomic(TypedAtomic::Load {
                    ty: type_arg.unwrap_or_else(|| self.error()),
                    ptr: Box::new(self.lower_expr(ptr)),
                    order: self.lower_atomic_order(order),
                })
            }
            (nia_ids::BuiltinFunction::AtomicStore, [ptr, value, order]) => {
                TypedExprKind::Atomic(TypedAtomic::Store {
                    ty: type_arg.unwrap_or_else(|| self.error()),
                    ptr: Box::new(self.lower_expr(ptr)),
                    value: Box::new(self.lower_expr(value)),
                    order: self.lower_atomic_order(order),
                })
            }
            (nia_ids::BuiltinFunction::AtomicRmw, [ptr, op, value, order]) => {
                TypedExprKind::Atomic(TypedAtomic::Rmw {
                    ty: type_arg.unwrap_or_else(|| self.error()),
                    ptr: Box::new(self.lower_expr(ptr)),
                    op: self.lower_atomic_rmw_op(op),
                    value: Box::new(self.lower_expr(value)),
                    order: self.lower_atomic_order(order),
                })
            }
            (
                nia_ids::BuiltinFunction::CmpxchgStrong,
                [ptr, expected, desired, success, failure],
            ) => TypedExprKind::Atomic(TypedAtomic::Cmpxchg {
                ty: type_arg.unwrap_or_else(|| self.error()),
                ptr: Box::new(self.lower_expr(ptr)),
                expected: Box::new(self.lower_expr(expected)),
                desired: Box::new(self.lower_expr(desired)),
                success: self.lower_atomic_order(success),
                failure: self.lower_atomic_order(failure),
                weak: false,
            }),
            (nia_ids::BuiltinFunction::CmpxchgWeak, [ptr, expected, desired, success, failure]) => {
                TypedExprKind::Atomic(TypedAtomic::Cmpxchg {
                    ty: type_arg.unwrap_or_else(|| self.error()),
                    ptr: Box::new(self.lower_expr(ptr)),
                    expected: Box::new(self.lower_expr(expected)),
                    desired: Box::new(self.lower_expr(desired)),
                    success: self.lower_atomic_order(success),
                    failure: self.lower_atomic_order(failure),
                    weak: true,
                })
            }
            (nia_ids::BuiltinFunction::Fence, [order]) => {
                TypedExprKind::Atomic(TypedAtomic::Fence {
                    order: self.lower_atomic_order(order),
                })
            }
            _ => TypedExprKind::Error,
        }
    }

    fn lower_builtin_value_expr(&mut self, expr: &Expr) -> TypedExprKind {
        match self.builtin_value(expr) {
            Some(BuiltinValue::Int(value)) => {
                TypedExprKind::BuiltinValue(BuiltinConst::Int(*value))
            }
            Some(BuiltinValue::Usize(value)) => {
                TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
            }
            Some(BuiltinValue::Layout { builtin, ty }) => {
                TypedExprKind::BuiltinValue(BuiltinConst::Layout {
                    builtin: *builtin,
                    ty: *ty,
                })
            }
            Some(BuiltinValue::FieldOffset { ty, field }) => {
                TypedExprKind::BuiltinValue(BuiltinConst::FieldOffset {
                    ty: *ty,
                    field: *field,
                })
            }
            None => TypedExprKind::Error,
        }
    }

    fn lower_atomic_order(&mut self, expr: &Expr) -> AtomicOrder {
        match self.lower_const_int(expr) {
            Some(0) => AtomicOrder::Unordered,
            Some(1) => AtomicOrder::Monotonic,
            Some(2) => AtomicOrder::Acquire,
            Some(3) => AtomicOrder::Release,
            Some(4) => AtomicOrder::AcqRel,
            Some(5) => AtomicOrder::SeqCst,
            _ => AtomicOrder::Monotonic,
        }
    }

    fn lower_atomic_rmw_op(&mut self, expr: &Expr) -> AtomicRmwOp {
        match self.lower_const_int(expr) {
            Some(0) => AtomicRmwOp::Xchg,
            Some(1) => AtomicRmwOp::Add,
            Some(2) => AtomicRmwOp::Sub,
            Some(3) => AtomicRmwOp::And,
            Some(4) => AtomicRmwOp::Nand,
            Some(5) => AtomicRmwOp::Or,
            Some(6) => AtomicRmwOp::Xor,
            Some(7) => AtomicRmwOp::Max,
            Some(8) => AtomicRmwOp::Min,
            Some(9) => AtomicRmwOp::UMax,
            Some(10) => AtomicRmwOp::UMin,
            _ => AtomicRmwOp::Xchg,
        }
    }

    fn lower_const_int(&mut self, expr: &Expr) -> Option<i128> {
        match self
            .with_const_context(|this| {
                let expr =
                    this.lower_const_expr(expr)
                        .map_err(|err| nia_const_eval::ConstError {
                            span: err.span,
                            message: err.message,
                        })?;
                nia_const_eval::eval_resolved_const_expr(&expr, this)
            })
            .ok()?
        {
            nia_const_eval::ConstValue::Int(value) => value.as_i128(),
            _ => None,
        }
    }

    fn runtime_ty_for_global_const_use(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        fallback: nia_ids::InternedTyId,
    ) -> nia_ids::InternedTyId {
        if def_id.module_id == self.defs.module_id {
            return self
                .typed_runtime_ty_for_current_module_const(def_id)
                .or_else(|| self.const_types.get(&def_id.def_id).copied())
                .unwrap_or(fallback);
        }
        if fallback != self.error() {
            return fallback;
        }
        self.qualified_program_const_type(def_id)
            .unwrap_or(fallback)
    }

    fn typed_runtime_ty_for_current_module_const(
        &mut self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_ids::InternedTyId> {
        let typed = self
            .const_eval
            .typed_values
            .get(&nia_const_check::ConstKey::Global(def_id))?
            .clone();
        let nia_const_check::ConstValueType::Runtime(ty) = typed.ty else {
            return None;
        };
        Some(ty)
    }

    pub(crate) fn expr_runtime_ty(&mut self, expr: &Expr) -> nia_ids::InternedTyId {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        if let Some(def_id) = self.global_const_use(expr) {
            return self.runtime_ty_for_global_const_use(def_id, ty);
        }
        if let Some(def_id) = self.qualified_value(expr)
            && matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
        {
            return self.runtime_ty_for_global_const_use(def_id, ty);
        }
        ty
    }

    fn lower_const_value_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: Option<nia_const_check::ConstValue>,
    ) -> TypedExpr {
        if ty == self.error() {
            return TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Error,
            };
        }
        match value {
            Some(nia_const_check::ConstValue::Int(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(value)),
            },
            Some(nia_const_check::ConstValue::Float(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Float(value.to_string()),
            },
            Some(nia_const_check::ConstValue::Bool(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Bool(value),
            },
            Some(value) => {
                if let Some(expr) = self.materialize_const_value_expr(span, ty, &value) {
                    return expr;
                }
                self.diagnostics
                    .push(nia_diagnostic::Diagnostic::user_error_at(
                        nia_diagnostic::codes::TYPE_CHECK,
                        span,
                        "runtime expression cannot use this const value",
                    ));
                TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Error,
                }
            }
            None => {
                self.diagnostics
                    .push(nia_diagnostic::Diagnostic::user_error_at(
                        nia_diagnostic::codes::TYPE_CHECK,
                        span,
                        "const value is not available during body check",
                    ));
                TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Error,
                }
            }
        }
    }

    fn materialize_const_value_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
    ) -> Option<TypedExpr> {
        let normalized_ty = self.normalization.normalize(ty);
        match self.interner.get(normalized_ty).cloned()? {
            TyKind::Pointer { is_readonly, elem } => {
                if !is_readonly {
                    return None;
                }
                let array =
                    self.materialize_const_array_expr(span, elem, pointer_pointee(value)?)?;
                let ty = if array.ty == elem {
                    ty
                } else {
                    self.interner.intern(TyKind::Pointer {
                        is_readonly: true,
                        elem: array.ty,
                    })
                };
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::StaticArrayPointer {
                        array: Box::new(array),
                        is_readonly: true,
                    },
                })
            }
            TyKind::Slice { is_readonly, elem } => {
                if !is_readonly {
                    return None;
                }
                let array_ty = self.const_array_ty_for_slice_value(elem, value)?;
                let array = self.materialize_const_array_expr(span, array_ty, value)?;
                let pointer = TypedExpr {
                    span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: true,
                        elem: array.ty,
                    }),
                    kind: TypedExprKind::StaticArrayPointer {
                        array: Box::new(array),
                        is_readonly: true,
                    },
                };
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Slice {
                        lhs: Box::new(pointer),
                        range: TypedSliceRange {
                            start: None,
                            end: None,
                            inclusive: false,
                        },
                        is_readonly: true,
                    },
                })
            }
            TyKind::Array { .. } => self.materialize_const_array_expr(span, ty, value),
            TyKind::Nominal { def_id, .. } if self.is_union_def(def_id) => {
                let nia_const_check::ConstValue::Union(union) = value else {
                    return None;
                };
                let name = union.last_written_field();
                let field_ty = self.field_ty_for_aggregate_ty(ty, &name)?;
                let field_value = union.read(name).ok()?;
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(TypedFieldInit {
                            field: self.field_def_for_aggregate_ty(ty, &name),
                            name: self.symbol_name(name),
                            value: self.lower_const_value_expr(span, field_ty, Some(field_value)),
                            span,
                        }),
                    },
                })
            }
            _ => None,
        }
    }

    fn const_array_ty_for_slice_value(
        &mut self,
        elem: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
    ) -> Option<nia_ids::InternedTyId> {
        let values = pointer_pointee(value).and_then(const_array_values)?;
        Some(self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(values.len() as u64),
            elem,
        }))
    }

    fn materialize_const_array_expr(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: &nia_const_check::ConstValue,
    ) -> Option<TypedExpr> {
        let normalized_ty = self.normalization.normalize(ty);
        let TyKind::Array { len, elem } = self.interner.get(normalized_ty).cloned()? else {
            return None;
        };
        let values = const_array_values(value)?;
        let ty = match len {
            ArrayLenTy::ConstValue(expected_len) => {
                if expected_len != values.len() as u64 {
                    return None;
                }
                ty
            }
            ArrayLenTy::Infer => self.interner.intern(TyKind::Array {
                len: ArrayLenTy::ConstValue(values.len() as u64),
                elem,
            }),
            ArrayLenTy::GenericParam(_) => return None,
            ArrayLenTy::ConstExpr(_) | ArrayLenTy::Builtin { .. } => {
                if self.array_len_value(span, &len).ok()? != values.len() as u64 {
                    return None;
                }
                ty
            }
        };
        let elem = self.normalization.normalize(elem);
        match self.interner.get(elem) {
            Some(TyKind::Primitive(PrimitiveTy::U8)) => {
                let bytes = const_ints_to_u8(values)?;
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ByteString(bytes),
                })
            }
            Some(TyKind::Primitive(PrimitiveTy::Char)) => {
                let scalars = const_ints_to_u32(values)?;
                if scalars
                    .iter()
                    .all(|scalar| char::from_u32(*scalar).is_some())
                {
                    Some(TypedExpr {
                        span,
                        ty,
                        kind: TypedExprKind::String(scalars),
                    })
                } else {
                    None
                }
            }
            _ => {
                let ConstArrayValues::Values(values) = values else {
                    return None;
                };
                let elems = values
                    .iter()
                    .cloned()
                    .map(|value| self.lower_const_value_expr(span, elem, Some(value)))
                    .collect();
                Some(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::ArrayLiteral {
                        elems: TypedArrayElements::List(elems),
                    },
                })
            }
        }
    }

    pub(crate) fn global_const_value(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_const_check::ConstValue> {
        self.global_const_value_for_env(def_id)
    }

    fn global_const_value_for_env(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_const_check::ConstValue> {
        let key = nia_const_check::ConstKey::Global(def_id);
        if def_id.module_id == self.defs.module_id {
            return self.const_eval.values.get(&key).cloned();
        }
        (self.program_const_values)(def_id.module_id)
            .and_then(|const_eval| const_eval.values.get(&key).cloned())
    }

    fn empty_struct_literal_expr(&mut self, ty: nia_ids::InternedTyId, block: &Block) -> bool {
        if !block.stmts.is_empty() || block.tail.is_some() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return false;
        };
        self.resolved_struct_signature(*def_id)
            .is_some_and(|resolved| resolved.signature.fields.is_empty())
    }

    fn lower_deref_pointer(&mut self, expr: &Expr) -> Option<TypedExpr> {
        let ty = self.expr_runtime_ty(expr);
        self.lower_builtin_deref_method_call(expr, ty, false)
    }

    fn lower_index_expr(&mut self, lhs: &Expr, index: &Expr) -> Option<TypedExprKind> {
        let lhs_ty = self.expr_runtime_ty(lhs);
        let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
        let pointer =
            self.lower_non_intrinsic_builtin_index_method_call(lhs, index, lhs_ty, index_ty)?;
        Some(TypedExprKind::Unary {
            op: UnaryOp::Deref,
            expr: Box::new(pointer),
        })
    }

    fn nominal_global_def(&self, ty: nia_ids::InternedTyId) -> Option<nia_ids::GlobalDefId> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
            _ => None,
        }
    }

    pub(crate) fn field_def_for_aggregate_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &SymbolId,
    ) -> Option<nia_ids::GlobalDefId> {
        let def_id = self.nominal_global_def(ty)?;
        self.field_def_for_nominal(def_id, name)
    }

    fn field_ty_for_aggregate_ty(
        &mut self,
        ty: nia_ids::InternedTyId,
        name: &SymbolId,
    ) -> Option<nia_ids::InternedTyId> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.interner.get(ty).cloned()
        else {
            return None;
        };
        let fields = if self.is_union_def(def_id) {
            self.resolved_union_signature(def_id)?.signature.fields
        } else {
            self.resolved_struct_signature(def_id)?.signature.fields
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let field = fields.iter().find(|field| &field.name == name)?;
        let ty =
            self.substitute_generics_and_consts(field.ty, &substitutions, &const_substitutions);
        Some(self.normalize_aliases_in_type(ty))
    }

    pub(crate) fn field_def_for_base_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &SymbolId,
    ) -> Option<nia_ids::GlobalDefId> {
        let base = self.receiver_base_type(ty)?;
        self.field_def_for_nominal(base.def_id, name)
    }

    fn lower_field_access_expr(&mut self, lhs: &Expr, name: &SymbolId) -> Option<TypedExprKind> {
        if let Some(kind) = self.lower_const_union_field_access(lhs, name) {
            return Some(kind);
        }
        let lhs_expr = self.lower_expr(lhs);
        let (lhs_expr, base_ty) = self.lower_field_lhs_to_value(lhs_expr)?;
        let field = self.field_def_for_aggregate_ty(base_ty, name)?;
        Some(TypedExprKind::Field {
            lhs: Box::new(lhs_expr),
            field,
        })
    }

    fn lower_const_union_field_access(
        &mut self,
        lhs: &Expr,
        name: &SymbolId,
    ) -> Option<TypedExprKind> {
        let def_id = self.global_const_use(lhs).or_else(|| {
            self.qualified_value(lhs).filter(|def_id| {
                matches!(
                    self.global_def_kind(*def_id),
                    Some(nia_defs::DefKind::Const)
                )
            })
        })?;
        let nia_const_check::ConstValue::Union(union) = self.global_const_value(def_id)? else {
            return None;
        };
        let fallback = self.expr_runtime_ty(lhs);
        let base_ty = self.runtime_ty_for_global_const_use(def_id, fallback);
        let field_ty = self.field_ty_for_aggregate_ty(base_ty, name)?;
        let value = union.read(*name).ok()?;
        Some(
            self.lower_const_value_expr(lhs.span, field_ty, Some(value))
                .kind,
        )
    }

    fn lower_field_lhs_to_value(
        &mut self,
        lhs: TypedExpr,
    ) -> Option<(TypedExpr, nia_ids::InternedTyId)> {
        let ty = self.normalize_aliases_in_type(lhs.ty);
        match self.interner.get(ty).cloned()? {
            TyKind::Nominal { .. } => Some((lhs, ty)),
            TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. } => {
                let elem = self.normalize_aliases_in_type(elem);
                Some((
                    TypedExpr {
                        span: lhs.span,
                        ty: elem,
                        kind: TypedExprKind::Unary {
                            op: UnaryOp::Deref,
                            expr: Box::new(lhs),
                        },
                    },
                    elem,
                ))
            }
            _ => None,
        }
    }

    pub(crate) fn field_def_for_nominal(
        &self,
        def_id: nia_ids::GlobalDefId,
        name: &SymbolId,
    ) -> Option<nia_ids::GlobalDefId> {
        let defs = self.defs_for_module(def_id.module_id)?;
        let defs = defs.as_ref();
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| nia_ids::GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    fn qualified_enum_variant(&self, expr: &Expr) -> Option<nia_ids::GlobalDefId> {
        self.variant_enum(expr)
            .and_then(|_| self.qualified_value(expr))
    }

    fn enum_variant_for_qualified(
        &mut self,
        lhs: &Expr,
        name: &SymbolId,
    ) -> Option<nia_ids::GlobalDefId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some(nia_ids::GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_id,
        })
    }

    fn lower_ident_expr(&mut self, expr: &Expr) -> TypedExprKind {
        match self.local_use(expr) {
            Some(LocalUse::Local(local)) => {
                if self
                    .locals
                    .locals
                    .get(local)
                    .is_some_and(|local| local.kind == LocalKind::ConstBinding)
                {
                    return TypedExprKind::Error;
                }
                TypedExprKind::Local(local)
            }
            Some(LocalUse::Static(global_id)) => TypedExprKind::Global(global_id),
            Some(LocalUse::ModuleValue) => {
                if let Some(variant_id) = self.qualified_value(expr)
                    && self.variant_enum(expr).is_some()
                {
                    return TypedExprKind::EnumVariant {
                        variant: variant_id,
                        fields: Vec::new(),
                    };
                }
                if let Some(global_id) = self.qualified_value(expr) {
                    match self.global_def_kind(global_id) {
                        Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                            return TypedExprKind::Function(global_id);
                        }
                        Some(nia_defs::DefKind::Global) => return TypedExprKind::Global(global_id),
                        Some(nia_defs::DefKind::Const) => return TypedExprKind::Error,
                        _ => return TypedExprKind::Error,
                    }
                }
                match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        match self.defs.defs.get(def_id).map(|def| def.kind) {
                            Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                                TypedExprKind::Function(self.global_def_id(def_id))
                            }
                            Some(nia_defs::DefKind::Global) => {
                                TypedExprKind::Global(self.global_def_id(def_id))
                            }
                            Some(nia_defs::DefKind::Const) => TypedExprKind::Error,
                            _ => TypedExprKind::Error,
                        }
                    }
                    _ => TypedExprKind::Error,
                }
            }
            Some(LocalUse::Module)
            | Some(LocalUse::TypePrefix)
            | Some(LocalUse::Unresolved)
            | None => match &expr.kind {
                ExprKind::Ident(name) => self
                    .current_const_generic_arg(name)
                    .map(TypedExprKind::ConstGeneric)
                    .unwrap_or(TypedExprKind::Error),
                _ => TypedExprKind::Error,
            },
        }
    }

    fn lower_function_item_ref(&mut self, expr: &Expr) -> Option<TypedExprKind> {
        let reference = self.function_reference(expr)?;
        if reference.args.is_empty() {
            Some(TypedExprKind::Function(reference.def_id))
        } else {
            Some(TypedExprKind::FunctionInstance {
                def_id: reference.def_id,
                arg_module_id: reference.arg_module_id,
                args: reference.args.clone(),
                const_args: reference.const_args.clone(),
            })
        }
    }

    fn lower_slice_range(&mut self, range: &SliceRange) -> TypedSliceRange {
        TypedSliceRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_range(&mut self, range: &SliceRange) -> TypedRange {
        TypedRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_array_elements(
        &mut self,
        elems: &ArrayElements,
        array_ty: nia_ids::InternedTyId,
    ) -> TypedArrayElements {
        let elem_ty = self.array_elem_ty(array_ty);
        match elems {
            ArrayElements::List(elems) => TypedArrayElements::List(
                elems
                    .iter()
                    .map(|elem| self.lower_expr_with_ty(elem, elem_ty))
                    .collect(),
            ),
            ArrayElements::Repeat { value, count } => TypedArrayElements::Repeat {
                value: Box::new(self.lower_expr_with_ty(value, elem_ty)),
                count: self.lower_array_repeat_len(count, array_ty),
            },
        }
    }

    fn lower_array_repeat_len(
        &mut self,
        count: &Expr,
        array_ty: nia_ids::InternedTyId,
    ) -> ArrayLenTy {
        match self.interner.get(self.normalization.normalize(array_ty)) {
            Some(TyKind::Array { len, .. }) if !matches!(len, ArrayLenTy::Infer) => {
                return len.clone();
            }
            _ => {}
        }
        ArrayLenTy::ConstValue(self.lower_array_repeat_count(count))
    }

    fn array_elem_ty(&self, array_ty: nia_ids::InternedTyId) -> Option<nia_ids::InternedTyId> {
        match self.interner.get(self.normalization.normalize(array_ty)) {
            Some(TyKind::Array { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(crate) fn lower_array_repeat_count(&mut self, count: &Expr) -> u64 {
        if let Some(value) = self.node_array_repeat_counts.get(&count.node_key).copied() {
            return value;
        }
        if let Ok(value) = self.eval_array_repeat_count(count) {
            self.record_array_repeat_count(count, value);
            return value;
        }
        self.diagnostics
            .push(nia_diagnostic::Diagnostic::user_error_at(
                nia_diagnostic::codes::TYPE_CHECK,
                count.span,
                "missing checked array repeat count during body IR lowering",
            ));
        0
    }

    fn lower_callee(&mut self, call: &Expr, callee: &Expr, args: &[Expr]) -> TypedCallee {
        if let Some(resolved) = self.resolved_call(call) {
            return self.lower_resolved_callee(callee, args, resolved);
        }
        if let Some(reference) = self.function_reference(callee) {
            if reference.args.is_empty() {
                return TypedCallee::Function(reference.def_id);
            }
            return TypedCallee::FunctionInstance {
                def_id: reference.def_id,
                arg_module_id: reference.arg_module_id,
                args: reference.args.clone(),
                const_args: reference.const_args.clone(),
            };
        }
        if let Some(def_id) = self.qualified_value(callee)
            && matches!(
                self.global_def_kind(def_id),
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method)
            )
        {
            return TypedCallee::Function(def_id);
        }
        if matches!(callee.kind, ExprKind::BracketSuffix { .. })
            && matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            )
        {
            return TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)));
        }
        if let ExprKind::Ident(_) = generic_inst_base(callee).kind
            && let Some(ValueNameResolution::Def(def_id)) =
                self.value_name(generic_inst_base(callee))
        {
            let def_id = self.global_def_id(def_id);
            if matches!(
                self.global_def_kind(def_id),
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method)
            ) {
                return TypedCallee::Function(def_id);
            }
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    fn lower_call_args(&mut self, call: &Expr, callee: &Expr, args: &[Expr]) -> Vec<TypedExpr> {
        let skip_first_arg = self
            .resolved_call(call)
            .is_some_and(|resolved| self.call_args_start_with_method_receiver(callee, resolved));
        let value_args = if skip_first_arg && !args.is_empty() {
            &args[1..]
        } else {
            args
        };
        let param_tys = self
            .resolved_call(call)
            .and_then(|resolved| self.lowered_explicit_call_arg_tys(callee, resolved))
            .or_else(|| self.lowered_call_param_tys_from_callee(callee));
        let Some(param_tys) = param_tys else {
            return value_args
                .iter()
                .map(|arg| self.lower_expr_with_checked_ty(arg))
                .collect();
        };
        value_args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                let expected = param_tys.get(index).copied().or_else(|| {
                    self.node_pointer_array_to_slice_coercions
                        .get(&arg.node_key)
                        .map(|coercion| coercion.slice_ty)
                });
                let expected = expected.and_then(|ty| self.non_error_ty(ty));
                self.lower_expr_with_ty(arg, expected)
            })
            .collect()
    }

    fn call_args_start_with_method_receiver(&self, callee: &Expr, resolved: ResolvedCall) -> bool {
        matches!(
            resolved,
            ResolvedCall::Method { .. }
                | ResolvedCall::TraitMethod { .. }
                | ResolvedCall::BuiltinFunction { .. }
                | ResolvedCall::BuiltinTraitMethod { .. }
                | ResolvedCall::BuiltinMethod { .. }
                | ResolvedCall::BuiltinPlaceMethod { .. }
        ) && !self.callee_has_receiver_lhs(callee)
    }

    fn lower_builtin_trait_method_call_args(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Vec<TypedExpr> {
        if let Some(receiver) = self.lower_receiver_expr(callee) {
            return std::iter::once(receiver)
                .chain(args.iter().map(|arg| self.lower_expr_with_checked_ty(arg)))
                .collect();
        }
        let Some((receiver, value_args)) = args.split_first() else {
            return Vec::new();
        };
        std::iter::once(self.lower_expr_with_checked_ty(receiver))
            .chain(
                value_args
                    .iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg)),
            )
            .collect()
    }

    fn lowered_explicit_call_arg_tys(
        &mut self,
        _callee: &Expr,
        resolved: ResolvedCall,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        let has_receiver_param = matches!(
            resolved,
            ResolvedCall::Method { .. } | ResolvedCall::TraitMethod { .. }
        );
        let params = self.lowered_call_param_tys(resolved)?;
        if has_receiver_param {
            Some(params.into_iter().skip(1).collect())
        } else {
            Some(params)
        }
    }

    fn lowered_call_param_tys_from_callee(
        &mut self,
        callee: &Expr,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        self.qualified_callee_signature(callee)
            .or_else(|| self.direct_callee_signature(callee))
            .map(|resolved| {
                resolved
                    .signature
                    .params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect()
            })
            .or_else(|| self.lowered_function_pointer_param_tys(callee))
    }

    fn lowered_function_pointer_param_tys(
        &mut self,
        callee: &Expr,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        let callee_ty = self.expr_ty(callee)?;
        let callee_ty = self.normalize_projection(callee_ty);
        match self.interner.get(callee_ty).cloned() {
            Some(TyKind::FunctionPointer { params, .. }) => Some(params),
            _ => None,
        }
    }

    fn lower_expr_with_checked_ty(&mut self, expr: &Expr) -> TypedExpr {
        if self.global_const_use(expr).is_some()
            || self.qualified_value(expr).is_some_and(|def_id| {
                matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
            })
        {
            let ty = self.expr_ty(expr);
            return self.lower_expr_with_ty(expr, ty);
        }
        self.lower_expr(expr)
    }

    fn lowered_call_param_tys(
        &mut self,
        resolved: ResolvedCall,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        match resolved {
            ResolvedCall::BuiltinFunction { .. } => None,
            ResolvedCall::Function(def_id) => Some(
                self.resolved_function_signature(def_id)?
                    .signature
                    .params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect(),
            ),
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: _,
                args,
                const_args,
            } => {
                let signature = self.resolved_function_signature(def_id)?.signature;
                let (mut substitutions, const_substitutions) =
                    self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
                if substitutions.len() < args.len() {
                    let effective_generics = self.effective_generics_for_def(def_id);
                    substitutions = self.generic_substitutions(&effective_generics, &args);
                }
                Some(
                    signature
                        .params
                        .into_iter()
                        .map(|param| {
                            self.substitute_generics_and_consts(
                                param.ty,
                                &substitutions,
                                &const_substitutions,
                            )
                        })
                        .collect(),
                )
            }
            ResolvedCall::Method {
                def_id,
                args,
                receiver_kind: _,
            } => {
                let signature = self.resolved_function_signature(def_id)?.signature;
                let mut substitutions = self.method_substitutions(def_id, &signature, &args);
                let receiver_ty = signature
                    .params
                    .first()
                    .and_then(|param| param.receiver)
                    .and_then(|receiver| {
                        let self_ty =
                            self.method_self_target_ty(def_id, &signature, &substitutions)?;
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver);
                        self.non_error_ty(receiver_ty)
                    });
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    receiver_ty,
                    receiver_ty,
                ))
            }
            ResolvedCall::TraitMethod {
                trait_id,
                method_id: _,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
            } => {
                let signature = self.trait_method_signature(trait_id, &method_name)?;
                let mut substitutions = self.generic_substitutions_for_trait_call(
                    trait_id,
                    &signature.generics,
                    &trait_args,
                    &args,
                );
                let receiver_ty = signature
                    .params
                    .first()
                    .and_then(|param| param.receiver)
                    .and_then(|receiver| {
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver);
                        self.non_error_ty(receiver_ty)
                    })
                    .or_else(|| {
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver_kind);
                        self.non_error_ty(receiver_ty)
                    });
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    receiver_ty,
                    Some(self_ty),
                ))
            }
            ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_id: _,
                method_name,
                self_ty,
                trait_args,
                args,
            } => {
                let signature = self.trait_method_signature(trait_id, &method_name)?;
                let mut substitutions = self.generic_substitutions_for_trait_call(
                    trait_id,
                    &signature.generics,
                    &trait_args,
                    &args,
                );
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    None,
                    Some(self_ty),
                ))
            }
            ResolvedCall::DynamicTraitMethod { params, .. } => Some(params),
            _ => None,
        }
    }

    fn method_substitutions(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        signature: &nia_item_signatures::FunctionSignature,
        args: &[nia_ids::InternedTyId],
    ) -> SymbolMap<nia_ids::InternedTyId> {
        let generics = self.effective_generics_for_def(def_id);
        let mut substitutions = self.generic_substitutions(&generics, args);
        for (generic, arg) in signature
            .generics
            .iter()
            .zip(args.iter().skip(generics.len()))
        {
            substitutions.entry(*generic).or_insert(*arg);
        }
        substitutions
    }

    fn method_self_target_ty(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        signature: &nia_item_signatures::FunctionSignature,
        substitutions: &SymbolMap<nia_ids::InternedTyId>,
    ) -> Option<nia_ids::InternedTyId> {
        if let Some(owner_ty) = self.method_owner_type_by_global(def_id) {
            let ty = self.substitute_generics(owner_ty, substitutions);
            let ty = self.normalize_projection(ty);
            let ty = self.normalize_aliases_in_type(ty);
            return self.non_error_ty(ty);
        }
        let ty = signature.params.first()?.ty;
        let ty = self.substitute_generics(ty, substitutions);
        self.non_error_ty(ty)
    }

    fn generic_substitutions_for_trait_call(
        &mut self,
        trait_id: nia_ids::GlobalDefId,
        method_generics: &[SymbolId],
        trait_args: &[nia_ids::InternedTyId],
        method_args: &[nia_ids::InternedTyId],
    ) -> SymbolMap<nia_ids::InternedTyId> {
        let trait_generics = self
            .resolved_trait_signature(trait_id)
            .map(|signature| signature.generics)
            .unwrap_or_default();
        let mut substitutions = self.generic_substitutions(&trait_generics, trait_args);
        substitutions.extend(self.generic_substitutions(method_generics, method_args));
        substitutions
    }

    fn trait_method_signature(
        &mut self,
        trait_id: nia_ids::GlobalDefId,
        method_name: &SymbolId,
    ) -> Option<nia_item_signatures::FunctionSignature> {
        self.resolved_trait_signature(trait_id)?
            .methods
            .into_iter()
            .find(|method| &method.name == method_name)
            .map(|method| method.signature)
    }

    fn substituted_call_param_tys(
        &mut self,
        params: &[nia_item_signatures::ParamSignature],
        substitutions: &mut SymbolMap<nia_ids::InternedTyId>,
        receiver_ty: Option<nia_ids::InternedTyId>,
        self_ty: Option<nia_ids::InternedTyId>,
    ) -> Vec<nia_ids::InternedTyId> {
        params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = if index == 0 {
                    receiver_ty.unwrap_or_else(|| match self_ty {
                        Some(self_ty) => {
                            self.substitute_generics_with_self(param.ty, substitutions, self_ty)
                        }
                        None => self.substitute_generics(param.ty, substitutions),
                    })
                } else {
                    match self_ty {
                        Some(self_ty) => {
                            self.substitute_generics_with_self(param.ty, substitutions, self_ty)
                        }
                        None => self.substitute_generics(param.ty, substitutions),
                    }
                };
                let ty = self.normalize_projection(ty);
                self.normalize_aliases_in_type(ty)
            })
            .collect()
    }

    fn lower_resolved_callee(
        &mut self,
        callee: &Expr,
        call_args: &[Expr],
        resolved: ResolvedCall,
    ) -> TypedCallee {
        match resolved {
            ResolvedCall::BuiltinFunction { .. } => {
                TypedCallee::FunctionPointer(Box::new(self.error_expr(callee.span)))
            }
            ResolvedCall::Function(def_id) => TypedCallee::Function(def_id),
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => TypedCallee::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            },
            ResolvedCall::Method {
                def_id,
                args,
                receiver_kind,
            } => {
                let receiver_ty = self
                    .lowered_call_param_tys(ResolvedCall::Method {
                        def_id,
                        args: args.clone(),
                        receiver_kind,
                    })
                    .and_then(|tys| tys.first().copied());
                TypedCallee::Method {
                    def_id,
                    args,
                    receiver_kind,
                    receiver: Box::new(self.lower_method_receiver_expr(
                        callee,
                        call_args,
                        receiver_kind,
                        receiver_ty,
                    )),
                }
            }
            ResolvedCall::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
            } => {
                let receiver_ty = self
                    .lowered_call_param_tys(ResolvedCall::TraitMethod {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args: trait_args.clone(),
                        args: args.clone(),
                        receiver_kind,
                    })
                    .and_then(|tys| tys.first().copied());
                TypedCallee::TraitMethod {
                    trait_id,
                    method_id,
                    method_name,
                    self_ty,
                    trait_args,
                    args,
                    receiver_kind,
                    receiver: Box::new(self.lower_method_receiver_expr(
                        callee,
                        call_args,
                        receiver_kind,
                        receiver_ty,
                    )),
                }
            }
            ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            } => TypedCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            },
            ResolvedCall::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
            } => TypedCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::BuiltinTraitMethod { trait_id, op, .. } => {
                TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op })
            }
            ResolvedCall::BuiltinMethod { method, self_ty } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinMethod {
                    method,
                    self_ty,
                    receiver: Box::new(receiver),
                }
            }
            ResolvedCall::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
            } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver: Box::new(
                        self.lower_typed_builtin_place_method_receiver(&receiver, self_ty, method),
                    ),
                })
            }
            ResolvedCall::FunctionPointer => {
                TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
            }
        }
    }

    fn lower_receiver_expr(&mut self, callee: &Expr) -> Option<TypedExpr> {
        self.lower_receiver_expr_with_ty(callee, None)
    }

    fn callee_has_receiver_lhs(&self, callee: &Expr) -> bool {
        match &callee.kind {
            ExprKind::Field { .. } => true,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                self.callee_has_receiver_lhs(generic_callee)
            }
            _ => false,
        }
    }

    fn lower_receiver_expr_with_ty(
        &mut self,
        callee: &Expr,
        expected: Option<nia_ids::InternedTyId>,
    ) -> Option<TypedExpr> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                generic_callee.as_ref()
            }
            _ => return None,
        };
        let lhs = match &field_callee.kind {
            ExprKind::Field { lhs, .. } => lhs,
            _ => return None,
        };
        Some(self.lower_expr_with_ty(lhs, expected))
    }

    fn lower_method_receiver_expr(
        &mut self,
        callee: &Expr,
        call_args: &[Expr],
        receiver_kind: ReceiverKind,
        receiver_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        let receiver_expr_ty =
            receiver_ty.map(|ty| self.receiver_expr_ty_for_lowering(callee, ty, receiver_kind));
        if self.callee_has_receiver_lhs(callee)
            && let Some(receiver) = self.lower_receiver_expr_with_ty(callee, receiver_expr_ty)
        {
            return receiver;
        }
        if let Some(receiver) = call_args.first() {
            return self.lower_expr_with_ty(receiver, receiver_expr_ty);
        }
        self.lower_expr_with_ty(callee, receiver_expr_ty)
    }

    fn receiver_expr_ty_for_lowering(
        &mut self,
        callee: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        receiver_kind: ReceiverKind,
    ) -> nia_ids::InternedTyId {
        match receiver_kind {
            ReceiverKind::Value => receiver_ty,
            ReceiverKind::RefReadOnly | ReceiverKind::Ref => {
                if let Some(actual_ty) = self.receiver_lhs_expr_ty(callee)
                    && self
                        .receiver_expr_already_matches_lowered_receiver_ty(receiver_ty, actual_ty)
                {
                    return receiver_ty;
                }
                match self.interner.get(self.normalization.normalize(receiver_ty)) {
                    Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
                        *elem
                    }
                    _ => receiver_ty,
                }
            }
        }
    }

    fn receiver_lhs_expr_ty(&mut self, callee: &Expr) -> Option<nia_ids::InternedTyId> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                generic_callee.as_ref()
            }
            _ => return None,
        };
        let ExprKind::Field { lhs, .. } = &field_callee.kind else {
            return None;
        };
        self.expr_ty(lhs)
    }

    fn receiver_expr_already_matches_lowered_receiver_ty(
        &mut self,
        receiver_ty: nia_ids::InternedTyId,
        actual_ty: nia_ids::InternedTyId,
    ) -> bool {
        if self.types_match(receiver_ty, actual_ty) {
            return true;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let actual_ty = self.normalization.normalize(actual_ty);
        match (
            self.interner.get(receiver_ty).cloned(),
            self.interner.get(actual_ty).cloned(),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                (expected_readonly || !actual_readonly)
                    && self.types_match(expected_elem, actual_elem)
            }
            _ => false,
        }
    }

    fn lower_builtin_call_receiver(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        receiver_ty: Option<nia_ids::InternedTyId>,
    ) -> (TypedExpr, Vec<TypedExpr>) {
        if let Some(receiver) = self.lower_receiver_expr_with_ty(callee, receiver_ty) {
            return (
                receiver,
                args.iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg))
                    .collect(),
            );
        }
        if let Some((receiver, args)) = args.split_first() {
            return (
                self.lower_expr_with_checked_ty(receiver),
                args.iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg))
                    .collect(),
            );
        }
        (self.lower_expr(callee), Vec::new())
    }

    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        let mut elems = Vec::new();
        let base = self.lower_place_inner(expr, &mut elems, true);
        TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_place_inner(
        &mut self,
        expr: &Expr,
        elems: &mut Vec<PlaceElem>,
        mutable: bool,
    ) -> PlaceBase {
        if self.variant_enum(expr).is_some() {
            return PlaceBase::Error;
        }
        if let Some(def_id) = self.qualified_value(expr)
            && !matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
        {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) | ExprKind::SelfValue => match self.local_use(expr) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(local),
                Some(LocalUse::Static(global_id)) => PlaceBase::Global(global_id),
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id))
                        if !matches!(
                            self.defs.defs.get(def_id).map(|def| def.kind),
                            Some(nia_defs::DefKind::Const)
                        ) =>
                    {
                        PlaceBase::Global(self.global_def_id(def_id))
                    }
                    _ => PlaceBase::Error,
                },
                _ => PlaceBase::Error,
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => {
                let ty = self.expr_runtime_ty(expr);
                if let Some(pointer) = self.lower_builtin_deref_method_call(expr, ty, mutable) {
                    PlaceBase::Deref(Box::new(pointer))
                } else {
                    PlaceBase::Deref(Box::new(self.lower_expr_with_ty(expr, Some(ty))))
                }
            }
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_place_inner(lhs, elems, mutable);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .map(PlaceElem::Field)
                    .unwrap_or(PlaceElem::Error);
                elems.push(field);
                base
            }
            ExprKind::Index { lhs, index } => {
                if let IndexArg::Expr(index) = index {
                    let lhs_ty = self.expr_runtime_ty(lhs);
                    let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                    if self.indirect_index_base(lhs).is_some() {
                        return self.lower_indirect_index_place_base(
                            expr, lhs, index, lhs_ty, index_ty, mutable,
                        );
                    }
                    if let Some(pointer) =
                        self.lower_builtin_index_method_call(lhs, index, lhs_ty, index_ty, mutable)
                    {
                        return PlaceBase::Deref(Box::new(pointer));
                    }
                    let base = self.lower_place_inner(lhs, elems, mutable);
                    elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                    return base;
                }
                PlaceBase::Error
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr),
                    Some(BracketSuffixResolution::Index)
                ) {
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        let lhs_ty = self.expr_ty(callee).unwrap_or_else(|| self.error());
                        let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                        if self.indirect_index_base(callee).is_some() {
                            return self.lower_indirect_index_place_base(
                                expr, callee, index, lhs_ty, index_ty, mutable,
                            );
                        }
                        if let Some(pointer) = self.lower_builtin_index_method_call(
                            callee, index, lhs_ty, index_ty, mutable,
                        ) {
                            return PlaceBase::Deref(Box::new(pointer));
                        }
                        let base = self.lower_place_inner(callee, elems, mutable);
                        elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                        return base;
                    }
                    PlaceBase::Error
                } else {
                    PlaceBase::Error
                }
            }
            _ => PlaceBase::Error,
        }
    }

    fn lower_builtin_deref_method_call(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, target_const) = if mutable {
            (BuiltinTrait::DerefMut, BuiltinTraitMethod::DerefMut, false)
        } else {
            (BuiltinTrait::Deref, BuiltinTraitMethod::Deref, true)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            Vec::new(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let target = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::TARGET,
        });
        let target = self.normalize_projection(target);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: target_const,
            elem: target,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args: Vec::new(),
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: Vec::new(),
            },
        })
    }

    fn lower_non_intrinsic_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
    ) -> Option<TypedExpr> {
        let trait_args = vec![index_ty];
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(BuiltinTrait::Index),
            trait_args.clone(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(BuiltinTrait::Index),
            trait_args: trait_args.clone(),
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: output,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id: BuiltinTrait::Index,
                    method: BuiltinTraitMethod::Index,
                    self_ty: receiver_ty,
                    trait_args,
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        BuiltinTraitMethod::Index,
                    )),
                }),
                args: vec![self.lower_expr_with_ty(index, Some(index_ty))],
            },
        })
    }

    fn lower_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, output_const) = if mutable {
            (BuiltinTrait::IndexMut, BuiltinTraitMethod::IndexMut, false)
        } else {
            (BuiltinTrait::Index, BuiltinTraitMethod::Index, true)
        };
        let trait_args = vec![index_ty];
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: trait_args.clone(),
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: output_const,
            elem: output,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args,
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: vec![self.lower_expr(index)],
            },
        })
    }

    fn lower_indirect_index_place_base(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> PlaceBase {
        let output_ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        let indexed = TypedExpr {
            span: expr.span,
            ty: output_ty,
            kind: TypedExprKind::Index {
                lhs: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                index: Box::new(self.lower_expr_with_ty(index, Some(index_ty))),
            },
        };
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: !mutable,
            elem: output_ty,
        });
        PlaceBase::Deref(Box::new(TypedExpr {
            span: expr.span,
            ty: pointer_ty,
            kind: TypedExprKind::Unary {
                op: if mutable {
                    UnaryOp::Ref
                } else {
                    UnaryOp::RefReadOnly
                },
                expr: Box::new(indexed),
            },
        }))
    }

    fn lower_slice_expr_readonly(&mut self, lhs: &Expr, range: &SliceRange) -> TypedExprKind {
        self.lower_slice_expr(lhs, range, true)
    }

    fn lower_slice_expr(
        &mut self,
        lhs: &Expr,
        range: &SliceRange,
        is_readonly: bool,
    ) -> TypedExprKind {
        let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
        let range_ty = self.check_slice_range_bounds(range);
        let (trait_id, method) = if is_readonly {
            (BuiltinTrait::Slice, BuiltinTraitMethod::Slice)
        } else {
            (BuiltinTrait::SliceMut, BuiltinTraitMethod::SliceMut)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            lhs_ty,
            TraitId::Builtin(trait_id),
            vec![range_ty],
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return TypedExprKind::Slice {
                lhs: Box::new(self.lower_expr(lhs)),
                range: self.lower_slice_range(range),
                is_readonly,
            };
        }
        TypedExprKind::Call {
            callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty: lhs_ty,
                trait_args: vec![range_ty],
                receiver: Box::new(self.lower_builtin_place_method_receiver(lhs, lhs_ty, method)),
            }),
            args: vec![self.lower_range_as_expr(range, range_ty)],
        }
    }

    fn lower_range_as_expr(&mut self, range: &SliceRange, ty: nia_ids::InternedTyId) -> TypedExpr {
        TypedExpr {
            span: self.slice_range_span(range),
            ty,
            kind: TypedExprKind::Range(self.lower_range(range)),
        }
    }

    fn slice_range_span(&self, range: &SliceRange) -> Span {
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Span::new(start.span.start, end.span.end),
            (Some(start), None) => start.span,
            (None, Some(end)) => end.span,
            (None, None) => Span::default(),
        }
    }

    fn lower_builtin_place_method_receiver(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            ReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                    },
                }
            }
            ReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                    },
                }
            }
            ReceiverKind::Value => self.lower_expr_with_ty(receiver, Some(receiver_ty)),
        }
    }

    fn lower_typed_builtin_place_method_receiver(
        &mut self,
        receiver: &TypedExpr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            ReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            ReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            ReceiverKind::Value => receiver.clone(),
        }
    }
}

fn pointer_pointee(value: &nia_const_check::ConstValue) -> Option<&nia_const_check::ConstValue> {
    match value {
        nia_const_check::ConstValue::Pointer(pointee) => Some(pointee),
        nia_const_check::ConstValue::String(_) => Some(value),
        _ => None,
    }
}

enum ConstArrayValues<'a> {
    Values(&'a [nia_const_check::ConstValue]),
    String(&'a str),
}

impl ConstArrayValues<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Values(values) => values.len(),
            Self::String(value) => value.chars().count(),
        }
    }
}

fn const_array_values(value: &nia_const_check::ConstValue) -> Option<ConstArrayValues<'_>> {
    match value {
        nia_const_check::ConstValue::Array(values) => Some(ConstArrayValues::Values(values)),
        nia_const_check::ConstValue::String(value) => Some(ConstArrayValues::String(value)),
        _ => None,
    }
}

fn const_ints_to_u8(values: ConstArrayValues<'_>) -> Option<Vec<u8>> {
    match values {
        ConstArrayValues::Values(values) => values
            .iter()
            .map(|value| {
                let nia_const_check::ConstValue::Int(value) = value else {
                    return None;
                };
                u8::try_from(value.bits()).ok()
            })
            .collect(),
        ConstArrayValues::String(_) => None,
    }
}

fn const_ints_to_u32(values: ConstArrayValues<'_>) -> Option<Vec<u32>> {
    match values {
        ConstArrayValues::Values(values) => values
            .iter()
            .map(|value| {
                let nia_const_check::ConstValue::Int(value) = value else {
                    return None;
                };
                u32::try_from(value.bits()).ok()
            })
            .collect(),
        ConstArrayValues::String(value) => Some(value.chars().map(u32::from).collect()),
    }
}
