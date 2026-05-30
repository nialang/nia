// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{
    byte_string_literal_len, c_string_literal_len, float_literal_text, has_numeric_literal_suffix,
    integer_literal_value, integer_range, numeric_literal_suffix, parse_float_literal,
    string_literal_char_len,
};
use nia_ast::{Expr, ExprKind, UnaryOp};
use nia_body_ir::{ArrayToSliceCoercion, CStringPointerCoercion};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, InternedTyId};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind};

impl<'a> BodyChecker<'a> {
    pub(crate) fn expect_type(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if expected == self.error() || actual == self.error() || self.types_match(expected, actual)
        {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "type mismatch in {context}: expected {}, got {}",
                self.ty_name(expected),
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn expect_expr_type(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if let Some(coerced) = self.coerce_c_string_to_pointer(expr, expected, actual) {
            self.record_expr_type(expr.span, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_array_to_slice(expr, expected, actual) {
            self.record_expr_type(expr.span, coerced);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_integer_literal_range(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_float_literal_target(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        self.expect_type(expr.span, expected, actual, context);
    }

    pub(crate) fn array_expected_from_slice_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        match self.interner.get(expected) {
            Some(TyKind::Slice { elem, .. }) => Some(self.interner.intern(TyKind::Array {
                len: ArrayLenTy::Infer,
                elem: *elem,
            })),
            _ => None,
        }
    }

    pub(crate) fn coerce_array_to_slice(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Slice {
            is_const,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_const = *is_const;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        let actual_elem = *actual_elem;
        if !self.types_match(expected_elem, actual_elem) {
            return None;
        }
        if self.is_place_expr(expr) {
            if is_const {
                self.check_addressable(expr, "array-to-slice source");
            } else {
                self.check_assignable(expr, "array-to-slice source");
            }
        }
        self.array_to_slice_coercions.insert(
            expr.span,
            ArrayToSliceCoercion {
                array_ty: actual,
                slice_ty: expected,
                is_const,
            },
        );
        Some(expected)
    }

    pub(crate) fn coerce_c_string_to_pointer(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        if !matches!(expr.kind, ExprKind::CString(_)) {
            return None;
        }
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Pointer {
            is_const,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_const = *is_const;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        if !self.types_match(expected_elem, *actual_elem)
            || !self.types_match(expected_elem, self.primitive(PrimitiveTy::U8))
        {
            return None;
        }
        self.c_string_pointer_coercions.insert(
            expr.span,
            CStringPointerCoercion {
                array_ty: actual,
                pointer_ty: expected,
                is_const,
            },
        );
        Some(expected)
    }

    fn materialize_literal_expr_type(&mut self, expr: &Expr, ty: InternedTyId) {
        self.record_expr_type(expr.span, ty);
        if let ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } = &expr.kind
        {
            self.record_expr_type(inner.span, ty);
        }
    }

    pub(crate) fn check_integer_literal_range(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} in {context}",
                    self.ty_name(expected)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_integer_literal_enum_backing_range(
        &mut self,
        expr: &Expr,
        expected_enum: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let Some(enum_id) = self.enum_global_def_id(expected_enum) else {
            return false;
        };
        let Some(signature) = self
            .resolved_enum_signature(enum_id)
            .map(|resolved| resolved.signature)
        else {
            return false;
        };
        let backing_type = signature.backing_type;
        let backing_type = self.normalization.normalize(backing_type);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(backing_type) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} backing type in {context}",
                    self.ty_name(expected_enum)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_float_literal_target(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(text) = float_literal_text(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        match primitive {
            PrimitiveTy::F32 => {
                if !parse_float_literal::<f32>(text) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("float literal `{text}` is out of range for F32 in {context}"),
                    ));
                }
                true
            }
            PrimitiveTy::F64 => {
                if !parse_float_literal::<f64>(text) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("float literal `{text}` is out of range for F64 in {context}"),
                    ));
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn numeric_literal_has_suffix(&self, expr: &Expr) -> bool {
        has_numeric_literal_suffix(expr)
    }

    pub(crate) fn report_invalid_numeric_literal_suffix(&mut self, expr: &Expr, kind: &str) {
        let suffix = numeric_literal_suffix_for_expr(expr).unwrap_or("<unknown>");
        self.diagnostics.push(Diagnostic::error(
            expr.span,
            format!("invalid {kind} literal suffix `{suffix}`"),
        ));
    }

    pub(crate) fn expect_integer(&mut self, span: Span, actual: InternedTyId, context: &str) {
        if actual == self.error() || self.is_integer(actual) {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "type mismatch in {context}: expected integer, got {}",
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn types_match(&self, expected: InternedTyId, actual: InternedTyId) -> bool {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        if self.is_never(actual) {
            return true;
        }
        if expected == actual {
            return true;
        }
        match (self.interner.get(expected), self.interner.get(actual)) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match(*expected_elem, *actual_elem) => true,
            (
                Some(TyKind::Array {
                    len: expected_len,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) if self.types_match(*expected_elem, *actual_elem) => {
                let Ok(expected_len) = self.array_len_value(Span::default(), expected_len) else {
                    return false;
                };
                let Ok(actual_len) = self.array_len_value(Span::default(), actual_len) else {
                    return false;
                };
                expected_len == actual_len
            }
            _ => false,
        }
    }

    pub(crate) fn materialize_inferred_array_type(
        &self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        match (self.interner.get(expected), self.interner.get(actual)) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match(*expected_elem, *actual_elem) => Some(actual),
            _ => None,
        }
    }

    pub(crate) fn def_id_for_span(&mut self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.defs.def_spans.get(span)?;
        let def = self.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn ty_for_span(&self, span: Span) -> InternedTyId {
        self.type_uses
            .get(&span)
            .copied()
            .unwrap_or_else(|| self.error())
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.normalization.normalize(ty);
        self.layouts.types.get(&ty).cloned()
    }

    pub(crate) fn array_len_value(&self, span: Span, len: &ArrayLenTy) -> Result<u64, String> {
        match len {
            ArrayLenTy::ConstValue(value) => Ok(*value),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .ok_or_else(|| "array length was not evaluated by comptime".to_string()),
            ArrayLenTy::Builtin { name, ty } => {
                let Some(layout) = self.layout_of(*ty) else {
                    return Err(format!(
                        "cannot compute layout for array length builtin `@{name}`"
                    ));
                };
                match name.as_str() {
                    "size" => Ok(layout.size),
                    "align" => Ok(layout.align),
                    _ => Err(format!("unsupported array length builtin `@{name}`")),
                }
            }
            ArrayLenTy::Infer => Err(format!("array length at {span:?} is not concrete")),
        }
    }

    pub(crate) fn ty_name(&self, ty: InternedTyId) -> String {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => primitive_ty_name(*primitive).to_string(),
            Some(TyKind::Pointer { is_const, elem }) => {
                let const_part = if *is_const { "const " } else { "" };
                format!("&{const_part}{}", self.ty_name(*elem))
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let const_part = if *is_const { "const " } else { "" };
                format!("&{const_part}[{}]", self.ty_name(*elem))
            }
            Some(TyKind::Array { len, elem }) => {
                format!("[{}]{}", self.array_len_name(len), self.ty_name(*elem))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let mut params = params
                    .iter()
                    .map(|param| self.ty_name(*param))
                    .collect::<Vec<_>>();
                if *is_variadic {
                    params.push("...".to_string());
                }
                let return_part = if self.is_void(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                format!("&const fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::Nominal { def_id, args }) => self.nominal_ty_name(*def_id, args),
            Some(TyKind::GenericParam(name)) => name.clone(),
            Some(TyKind::Error) => "<error type>".to_string(),
            None => "<unknown type>".to_string(),
        }
    }

    fn array_len_name(&self, len: &ArrayLenTy) -> String {
        match len {
            ArrayLenTy::Infer => "_".to_string(),
            ArrayLenTy::ConstValue(value) => value.to_string(),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<unevaluated const>".to_string()),
            ArrayLenTy::Builtin { name, ty } => format!("@{name}[{}]()", self.ty_name(*ty)),
        }
    }

    fn array_len_const_expr_value(&self, id: GlobalConstExprId) -> Option<u64> {
        if id.module_id == self.defs.module_id {
            return self.comptime.array_lengths.get(&id).copied();
        }
        self.program_comptime
            .get(&id.module_id)
            .and_then(|comptime| comptime.array_lengths.get(&id).copied())
    }

    fn nominal_ty_name(&self, def_id: nia_ids::GlobalDefId, args: &[InternedTyId]) -> String {
        let base = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .map(|def| def.name.clone())
            .unwrap_or_else(|| "<unknown type>".to_string());
        if args.is_empty() {
            base
        } else {
            let args = args
                .iter()
                .map(|arg| self.ty_name(*arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}[{args}]")
        }
    }

    pub(crate) fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.interner.primitive(primitive)
    }

    pub(crate) fn string_literal_type(&mut self, literal: &nia_ast::StringLiteral) -> InternedTyId {
        let len = string_literal_char_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::Char),
        })
    }

    pub(crate) fn byte_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = byte_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn c_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = c_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn void(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Void)
    }

    pub(crate) fn never(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Never)
    }

    pub(crate) fn is_void(&self, ty: InternedTyId) -> bool {
        ty == self.void()
    }

    pub(crate) fn is_never(&self, ty: InternedTyId) -> bool {
        self.normalization.normalize(ty) == self.never()
    }

    pub(crate) fn is_invalid_temporary_type(&self, ty: InternedTyId) -> bool {
        self.is_void(ty) || self.is_never(ty)
    }

    pub(crate) fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    pub(crate) fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    pub(crate) fn is_u32(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        )
    }

    pub(crate) fn is_numeric(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.interner.get(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
            )
    }

    pub(crate) fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }

    pub(crate) fn is_pointer_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Usize | PrimitiveTy::Isize))
        )
    }

    pub(crate) fn is_enum(&self, ty: InternedTyId) -> bool {
        self.enum_global_def_id(ty).is_some()
    }

    pub(crate) fn is_open_enum(&self, ty: InternedTyId) -> bool {
        let Some(enum_id) = self.enum_global_def_id(ty) else {
            return false;
        };
        if enum_id.module_id == self.defs.module_id {
            self.signatures
                .enums
                .get(&enum_id.def_id)
                .is_some_and(|signature| signature.is_open)
        } else {
            self.program_enums
                .get(&enum_id)
                .is_some_and(|program_enum| program_enum.signature.is_open)
        }
    }

    pub(crate) fn bool(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Bool)
    }

    pub(crate) fn i32(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::I32)
    }

    pub(crate) fn f64(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::F64)
    }

    pub(crate) fn error(&self) -> InternedTyId {
        self.interner.error()
    }
}

fn numeric_literal_suffix_for_expr(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Float(text) => numeric_literal_suffix(text),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => numeric_literal_suffix_for_expr(expr),
        _ => None,
    }
}

fn primitive_ty_name(primitive: PrimitiveTy) -> &'static str {
    match primitive {
        PrimitiveTy::I8 => "i8",
        PrimitiveTy::I16 => "i16",
        PrimitiveTy::I32 => "i32",
        PrimitiveTy::I64 => "i64",
        PrimitiveTy::I128 => "i128",
        PrimitiveTy::Isize => "isize",
        PrimitiveTy::U8 => "u8",
        PrimitiveTy::U16 => "u16",
        PrimitiveTy::U32 => "u32",
        PrimitiveTy::U64 => "u64",
        PrimitiveTy::U128 => "u128",
        PrimitiveTy::Usize => "usize",
        PrimitiveTy::F32 => "f32",
        PrimitiveTy::F64 => "f64",
        PrimitiveTy::Bool => "bool",
        PrimitiveTy::Char => "char",
        PrimitiveTy::Void => "void",
        PrimitiveTy::Never => "!",
    }
}
