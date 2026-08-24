// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{
    decode_byte_char_literal, decode_byte_string_literal, decode_char_literal,
    decode_string_literal, numeric_literal_body, parse_int_literal,
};
use nia_ast::{ArrayElements, Expr, ExprKind, IndexArg};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_local_resolve::LocalUse;
use nia_sema_ir::BuiltinValue;
use nia_static_ir::{StaticAddressElem, StaticFieldInit, StaticInit};
use nia_symbol::SymbolId;
use nia_ty::{IntConst, TyKind};
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    /// Lower a static initializer only when this lowering pass accepts it.
    ///
    /// The low-level lowering routines deliberately return `StaticInit::Zero`
    /// as a recovery value so aggregate traversal can continue reporting
    /// independent errors. That placeholder is not a valid product, however:
    /// publishing it would leak a fake initializer into reachability or
    /// executable Body IR. Treat both new diagnostics and any recovery value in
    /// the initializer tree as transaction failure.
    pub(crate) fn lower_global_static_init_checked(
        &mut self,
        expr: &Expr,
        ty: InternedTyId,
    ) -> Option<StaticInit> {
        let diagnostics_before = self.diagnostics.len();
        let init = self.lower_global_static_init(expr, ty);
        (self.diagnostics.len() == diagnostics_before && !Self::contains_static_recovery(&init))
            .then_some(init)
    }

    fn contains_static_recovery(init: &StaticInit) -> bool {
        match init {
            StaticInit::Zero => true,
            StaticInit::Array(values) => values.iter().any(Self::contains_static_recovery),
            StaticInit::Repeat { value, .. } => Self::contains_static_recovery(value),
            StaticInit::Struct(fields) => fields
                .iter()
                .any(|field| Self::contains_static_recovery(&field.value)),
            StaticInit::Int(_)
            | StaticInit::Float(_)
            | StaticInit::Bool(_)
            | StaticInit::Char(_)
            | StaticInit::Byte(_)
            | StaticInit::Chars(_)
            | StaticInit::Bytes(_)
            | StaticInit::NullPtr
            | StaticInit::AddrOfGlobal { .. }
            | StaticInit::AddrOfFunction { .. } => false,
        }
    }

    pub(crate) fn lower_global_static_init(&mut self, expr: &Expr, ty: InternedTyId) -> StaticInit {
        match &expr.kind {
            ExprKind::String(literal) if self.static_init_target_is_array(ty) => {
                self.lower_static_string_array_init(expr, literal)
            }
            ExprKind::ByteString(literal) if self.static_init_target_is_array(ty) => {
                self.lower_static_byte_string_array_init(expr, literal)
            }
            ExprKind::String(_) | ExprKind::ByteString(_) => {
                self.lower_static_string_target_mismatch(expr)
            }
            _ => self.lower_static_init_with_target(expr, ty),
        }
    }

    pub(crate) fn lower_static_init(&mut self, expr: &Expr) -> StaticInit {
        match &expr.kind {
            ExprKind::Integer(text) => parse_int_literal(text)
                .map(|value| StaticInit::Int(IntConst::unsigned(value)))
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("invalid integer literal `{text}` in static initializer"),
                    ));
                    StaticInit::Zero
                }),
            ExprKind::Float(text) => StaticInit::Float(numeric_literal_body(text).to_string()),
            ExprKind::Bool(value) => StaticInit::Bool(*value),
            ExprKind::String(literal) => self.lower_static_string_array_init(expr, literal),
            ExprKind::ByteString(literal) => {
                self.lower_static_byte_string_array_init(expr, literal)
            }
            ExprKind::Char(text) => decode_char_literal(text)
                .map(StaticInit::Char)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("invalid char literal `{text}` in static initializer"),
                    ));
                    StaticInit::Char(0)
                }),
            ExprKind::ByteChar(text) => decode_byte_char_literal(text)
                .map(StaticInit::Byte)
                .unwrap_or_else(|| {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!("invalid byte char literal `{text}` in static initializer"),
                    ));
                    StaticInit::Byte(0)
                }),
            ExprKind::Ident(_) | ExprKind::Qualified { .. } => {
                if let Some(BuiltinValue::Int(value)) = self.builtin_value(expr) {
                    return StaticInit::Int(*value);
                }
                if let Some(BuiltinValue::Usize(value)) = self.builtin_value(expr) {
                    return StaticInit::Int(IntConst::unsigned(*value as u128));
                }
                if let Some(value) = self.static_const_value(expr)
                    && let Some(init) = Self::lower_static_scalar_const_value(value)
                {
                    return init;
                }
                StaticInit::Zero
            }
            ExprKind::ArrayLiteral { elems } => match elems {
                ArrayElements::List(elems) => {
                    let elem_ty = self
                        .expr_ty(expr)
                        .and_then(|ty| self.static_array_elem_ty(ty));
                    StaticInit::Array(
                        elems
                            .iter()
                            .map(|elem| {
                                elem_ty
                                    .map(|ty| self.lower_static_init_with_target(elem, ty))
                                    .unwrap_or_else(|| self.lower_static_init(elem))
                            })
                            .collect(),
                    )
                }
                ArrayElements::Repeat { value, count } => StaticInit::Repeat {
                    value: Box::new(
                        self.expr_ty(expr)
                            .and_then(|ty| self.static_array_elem_ty(ty))
                            .map(|ty| self.lower_static_init_with_target(value, ty))
                            .unwrap_or_else(|| self.lower_static_init(value)),
                    ),
                    count: self.lower_array_repeat_count(count),
                },
            },
            ExprKind::TypedStructLiteral { fields, .. }
            | ExprKind::QualifiedStructLiteral { fields, .. } => {
                // Static aggregate layout is always driven by the nominal type
                // recorded for the checked expression, never by field shape.
                let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
                StaticInit::Struct(
                    fields
                        .iter()
                        .map(|field| StaticFieldInit {
                            field: self.field_def_for_aggregate_ty(ty, &field.name),
                            value: self
                                .static_field_ty(ty, &field.name)
                                .map(|field_ty| {
                                    self.lower_static_init_with_target(&field.value, field_ty)
                                })
                                .unwrap_or_else(|| self.lower_static_init(&field.value)),
                        })
                        .collect(),
                )
            }
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Neg,
                expr: inner,
            } if let ExprKind::Float(text) = &inner.kind => {
                StaticInit::Float(format!("-{}", numeric_literal_body(text)))
            }
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Neg,
                ..
            }
            | ExprKind::Binary { .. } => self
                .eval_static_const_int_expr(expr)
                .map(StaticInit::Int)
                .unwrap_or_else(|err| {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        format!(
                            "invalid integer constant in static initializer: {}",
                            err.message
                        ),
                    ));
                    StaticInit::Zero
                }),
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr: inner,
            } => {
                let _ = inner;
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "global initializer is not representable as static data yet",
                ));
                StaticInit::Zero
            }
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Ref | nia_ast::UnaryOp::RefReadOnly,
                expr,
            } => self.lower_static_address_init(expr),
            ExprKind::Cast { expr: inner, .. } => self.lower_static_cast_init(expr, inner),
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "global initializer is not representable as static data yet",
                ));
                StaticInit::Zero
            }
        }
    }

    fn lower_static_init_with_target(&mut self, expr: &Expr, ty: InternedTyId) -> StaticInit {
        match &expr.kind {
            ExprKind::String(literal) if self.static_init_target_is_array(ty) => {
                self.lower_static_string_array_init(expr, literal)
            }
            ExprKind::ByteString(literal) if self.static_init_target_is_array(ty) => {
                self.lower_static_byte_string_array_init(expr, literal)
            }
            ExprKind::String(_) | ExprKind::ByteString(_) => {
                self.lower_static_string_target_mismatch(expr)
            }
            ExprKind::Cast { expr: inner, .. } => {
                let cast_ty = self.expr_ty(expr).unwrap_or(ty);
                self.lower_static_cast_init_with_target(expr, inner, cast_ty)
            }
            _ => {
                let init = self.lower_static_init(expr);
                self.finish_static_target_init(init, ty)
            }
        }
    }

    /// Applies the checked initializer's integer signedness without changing
    /// its complete bit pattern. Explicit casts use
    /// [`Self::finish_static_cast_init`] and are the only boundary that masks
    /// to the destination width.
    fn finish_static_target_init(
        &mut self,
        init: StaticInit,
        target_ty: InternedTyId,
    ) -> StaticInit {
        let Some(TyKind::Primitive(primitive)) = self.interner.get(target_ty) else {
            return init;
        };
        let StaticInit::Int(value) = init else {
            return init;
        };
        let value = if primitive.is_signed_integer() {
            IntConst::signed_bits(value.bits())
        } else if primitive.is_integer() {
            IntConst::unsigned(value.bits())
        } else {
            return StaticInit::Int(value);
        };
        StaticInit::Int(value)
    }

    fn lower_static_string_array_init(
        &mut self,
        expr: &Expr,
        literal: &nia_ast::StringLiteral,
    ) -> StaticInit {
        decode_string_literal(literal)
            .map(StaticInit::Chars)
            .unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "invalid string literal in static initializer",
                ));
                StaticInit::Chars(Vec::new())
            })
    }

    fn lower_static_byte_string_array_init(
        &mut self,
        expr: &Expr,
        literal: &nia_ast::StringLiteral,
    ) -> StaticInit {
        decode_byte_string_literal(literal)
            .map(StaticInit::Bytes)
            .unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "invalid byte string literal in static initializer",
                ));
                StaticInit::Bytes(Vec::new())
            })
    }

    fn lower_static_string_target_mismatch(&mut self, expr: &Expr) -> StaticInit {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            expr.span,
            "string literal static initializer requires an array target",
        ));
        StaticInit::Zero
    }

    fn static_init_target_is_array(&mut self, ty: InternedTyId) -> bool {
        let ty = self.normalization.normalize(ty);
        matches!(self.interner.get(ty), Some(TyKind::Array { .. }))
    }

    fn static_array_elem_ty(&mut self, ty: InternedTyId) -> Option<InternedTyId> {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Array { elem, .. }) => Some(elem),
            _ => None,
        }
    }

    fn static_field_ty(&mut self, ty: InternedTyId, name: &SymbolId) -> Option<InternedTyId> {
        let ty = self.normalization.normalize(ty);
        let TyKind::Nominal { def_id, args, .. } = self.interner.get(ty).cloned()? else {
            return None;
        };
        let resolved = if self.is_union_def(def_id) {
            self.resolved_union_signature(def_id)?
                .signature
                .as_struct_like()
        } else {
            self.resolved_struct_signature(def_id)?.signature
        };
        let substitutions = self.generic_substitutions(&resolved.generics, &args);
        resolved
            .fields
            .iter()
            .find(|field| &field.name == name)
            .map(|field| self.substitute_generics(field.ty, &substitutions))
    }

    fn eval_static_const_int_expr(
        &mut self,
        expr: &Expr,
    ) -> Result<IntConst, nia_const_eval::ConstError> {
        let expr = self.eval_static_const_expr(expr)?;
        nia_const_eval::eval_resolved_const_int_expr(&expr, self)
    }

    fn eval_static_const_array_len_expr(
        &mut self,
        expr: &Expr,
    ) -> Result<u64, nia_const_eval::ConstError> {
        let expr = self.eval_static_const_expr(expr)?;
        nia_const_eval::eval_resolved_const_array_len_expr(&expr, self)
    }

    fn eval_static_const_expr(
        &mut self,
        expr: &Expr,
    ) -> Result<nia_const_ir::ResolvedConstExpr, nia_const_eval::ConstError> {
        self.with_const_context(|this| {
            this.lower_const_expr(expr)
                .map_err(|err| nia_const_eval::ConstError {
                    span: err.span,
                    message: err.message,
                })
        })
    }

    fn lower_static_cast_init(&mut self, cast: &Expr, inner: &Expr) -> StaticInit {
        let init = self.lower_static_init(inner);
        let Some(target_ty) = self.expr_ty(cast) else {
            return init;
        };
        self.finish_static_cast_init(cast, init, target_ty)
    }

    fn lower_static_cast_init_with_target(
        &mut self,
        cast: &Expr,
        inner: &Expr,
        target_ty: InternedTyId,
    ) -> StaticInit {
        let init = self.lower_static_init_with_target(inner, target_ty);
        self.finish_static_cast_init(cast, init, target_ty)
    }

    fn finish_static_cast_init(
        &mut self,
        _cast: &Expr,
        init: StaticInit,
        target_ty: InternedTyId,
    ) -> StaticInit {
        let Some(TyKind::Primitive(primitive)) = self.interner.get(target_ty) else {
            return init;
        };
        let StaticInit::Int(value) = init else {
            return init;
        };
        value
            .cast_to_primitive_int(*primitive, self.target.pointer_width)
            .map(StaticInit::Int)
            .unwrap_or(StaticInit::Int(value))
    }

    fn lower_static_address_init(&mut self, expr: &Expr) -> StaticInit {
        if let Some((function, args, const_args)) = self.static_function_address(expr) {
            return StaticInit::AddrOfFunction {
                function,
                args,
                const_args,
            };
        }
        let place = self.lower_static_place(expr);
        match place.base {
            StaticAddressBase::Global(global) => StaticInit::AddrOfGlobal {
                global,
                path: place.path,
            },
            StaticAddressBase::Invalid => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "global address initializer must refer to global storage",
                ));
                StaticInit::Zero
            }
        }
    }

    fn static_function_address(
        &self,
        expr: &Expr,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        self.function_reference(expr).map(|reference| {
            (
                reference.def_id,
                reference.args.clone(),
                reference.const_args.clone(),
            )
        })
    }

    fn lower_static_place(&mut self, expr: &Expr) -> StaticAddressPlace {
        let mut elems = Vec::new();
        let base = self.lower_static_place_inner(expr, &mut elems);
        StaticAddressPlace { base, path: elems }
    }

    fn lower_static_place_inner(
        &mut self,
        expr: &Expr,
        elems: &mut Vec<StaticAddressElem>,
    ) -> StaticAddressBase {
        if self.variant_enum(expr).is_some() {
            return StaticAddressBase::Invalid;
        }
        if let Some(def_id) = self.qualified_value(expr) {
            return StaticAddressBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.local_use(expr) {
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        StaticAddressBase::Global(self.global_def_id(def_id))
                    }
                    _ => StaticAddressBase::Invalid,
                },
                _ => StaticAddressBase::Invalid,
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "global address initializer cannot dereference runtime pointers",
                ));
                StaticAddressBase::Invalid
            }
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_static_place_inner(lhs, elems);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .map(StaticAddressElem::Field)
                    .unwrap_or(StaticAddressElem::Error);
                elems.push(field);
                base
            }
            ExprKind::Index { lhs, index } => {
                let base = self.lower_static_place_inner(lhs, elems);
                if let IndexArg::Expr(index) = index {
                    elems.push(StaticAddressElem::Index(
                        self.lower_static_place_index(index),
                    ));
                }
                base
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr),
                    Some(nia_sema_ir::BracketSuffixResolution::Index)
                ) {
                    let base = self.lower_static_place_inner(callee, elems);
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        elems.push(StaticAddressElem::Index(
                            self.lower_static_place_index(index),
                        ));
                    }
                    base
                } else {
                    StaticAddressBase::Invalid
                }
            }
            _ => StaticAddressBase::Invalid,
        }
    }

    fn lower_static_place_index(&mut self, expr: &Expr) -> u64 {
        match self.eval_static_const_array_len_expr(expr) {
            Ok(value) => value,
            Err(error) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!(
                        "static address index is not a valid usize constant: {}",
                        error.message
                    ),
                ));
                0
            }
        }
    }

    fn static_const_value(&self, expr: &Expr) -> Option<nia_const_check::ConstValue> {
        if let Some(global_id) = self.global_const_use(expr) {
            return self.global_const_value(global_id);
        }
        if let Some(local_id) = self.local_const_use(expr) {
            return Some(
                self.const_eval
                    .values
                    .get(&nia_const_check::ConstKey::Local(local_id))?
                    .clone(),
            );
        }
        None
    }

    fn lower_static_scalar_const_value(value: nia_const_check::ConstValue) -> Option<StaticInit> {
        match value {
            nia_const_check::ConstValue::Int(value) => Some(StaticInit::Int(value)),
            nia_const_check::ConstValue::Float(value) => Some(StaticInit::Float(value.to_string())),
            nia_const_check::ConstValue::Bool(value) => Some(StaticInit::Bool(value)),
            _ => None,
        }
    }
}

struct StaticAddressPlace {
    base: StaticAddressBase,
    path: Vec<StaticAddressElem>,
}

enum StaticAddressBase {
    Global(GlobalDefId),
    Invalid,
}
