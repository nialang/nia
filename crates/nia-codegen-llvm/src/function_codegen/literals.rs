// SPDX-License-Identifier: GPL-3.0-or-later
use crate::literals::{decode_byte_char_literal, parse_float_literal, parse_int_literal};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_llvm::values::BasicValueEnum;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::{PrimitiveTy, TyKind};

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_float_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        text: &str,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = parse_float_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid float literal `{text}`")))?;
        match self.module.ty_kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::F32)) => {
                Ok(self.module.context.f32_type().const_float(value).into())
            }
            Some(TyKind::Primitive(PrimitiveTy::F64)) => {
                Ok(self.module.context.f64_type().const_float(value).into())
            }
            _ => Err(self.error(span, "float literal target type is not float")),
        }
    }

    pub(super) fn emit_integer_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        text: &str,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = parse_int_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid integer literal `{text}`")))?;
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "integer literal target type is not primitive"));
        };
        let int_ty = self.integer_llvm_type(*primitive, span)?;
        Ok(int_ty.const_u128(value as u128).into())
    }

    pub(super) fn emit_string_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        scalars: &[u32],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "string literal target type is not an array"));
        };
        let values = scalars
            .iter()
            .map(|scalar| {
                self.module
                    .static_init_value_in_current(*elem, &StaticInit::Char(*scalar), span)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.module
            .const_array_from_values_in_current(*elem, &values, span)
    }

    pub(super) fn emit_byte_string_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        bytes: &[u8],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "byte string literal target type is not an array"));
        };
        let values = bytes
            .iter()
            .map(|byte| {
                self.module
                    .static_init_value_in_current(*elem, &StaticInit::Byte(*byte), span)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.module
            .const_array_from_values_in_current(*elem, &values, span)
    }

    pub(super) fn emit_char_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        value: u32,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "char literal target type is not primitive"));
        };
        let int_ty = self.integer_llvm_type(*primitive, span)?;
        Ok(int_ty.const_u128(value as u128).into())
    }

    pub(super) fn emit_byte_char_literal(
        &self,
        ty: InternedTyId,
        span: Span,
        text: &str,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = decode_byte_char_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid byte char literal `{text}`")))?;
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "byte char literal target type is not primitive"));
        };
        let int_ty = self.integer_llvm_type(*primitive, span)?;
        Ok(int_ty.const_u128(value as u128).into())
    }
}
