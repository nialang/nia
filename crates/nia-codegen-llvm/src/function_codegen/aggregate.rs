// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionErrorUnionTag, FunctionExpr, FunctionFieldInit,
    FunctionOptionalTag,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_llvm::values::{BasicValueEnum, PointerValue};
use nia_span::Span;
use nia_ty::TyKind;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_array_literal(
        &mut self,
        expr: &FunctionExpr,
        elems: &FunctionArrayElements,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let array_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(array_ty, "arraytmp")
            .map_err(|_| self.error(expr.span, "failed to allocate array literal"))?;
        self.emit_array_literal_into(expr, elems, ptr)?;
        self.builder
            .build_load(array_ty, ptr, "arraylit")
            .map_err(|_| self.error(expr.span, "failed to load array literal"))
    }

    pub(super) fn emit_array_literal_into(
        &mut self,
        expr: &FunctionExpr,
        elems: &FunctionArrayElements,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        match elems {
            FunctionArrayElements::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    let elem_ptr =
                        self.emit_const_index_addr(expr.span, expr.ty, ptr, index as u64)?;
                    let value = self.emit_expr(value)?;
                    self.builder
                        .build_store(elem_ptr, value)
                        .map_err(|_| self.error(expr.span, "failed to store array element"))?;
                }
            }
            FunctionArrayElements::Repeat { value, count } => {
                for index in 0..*count {
                    let elem_ptr = self.emit_const_index_addr(expr.span, expr.ty, ptr, index)?;
                    let value = self.emit_expr(value)?;
                    self.builder.build_store(elem_ptr, value).map_err(|_| {
                        self.error(expr.span, "failed to store repeated array element")
                    })?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn emit_struct_literal(
        &mut self,
        expr: &FunctionExpr,
        _def_id: GlobalDefId,
        fields: &[FunctionFieldInit],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if self
            .module
            .layout_of(expr.ty)
            .is_some_and(|layout| layout.size == 0)
        {
            return Err(self.error(expr.span, "zero-sized struct has no runtime value"));
        }
        let struct_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(struct_ty, "structtmp")
            .map_err(|_| self.error(expr.span, "failed to allocate struct literal"))?;
        self.emit_struct_literal_into(expr, fields, ptr)?;
        self.builder
            .build_load(struct_ty, ptr, "structlit")
            .map_err(|_| self.error(expr.span, "failed to load struct literal"))
    }

    pub(super) fn emit_struct_literal_into(
        &mut self,
        expr: &FunctionExpr,
        fields: &[FunctionFieldInit],
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let struct_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        for field in fields {
            let Some(field_id) = field.field else {
                return Err(self.error(field.span, "invalid struct field initializer"));
            };
            let field_index = self.field_index(expr.ty, field_id, field.span)?;
            let field_ptr = self
                .builder
                .build_struct_gep(struct_ty, ptr, field_index, "fieldptr")
                .map_err(|_| self.error(field.span, "failed to build struct field address"))?;
            let value = self.emit_expr(&field.value)?;
            self.builder
                .build_store(field_ptr, value)
                .map_err(|_| self.error(field.span, "failed to store struct field"))?;
        }
        Ok(())
    }

    pub(super) fn emit_union_literal(
        &mut self,
        expr: &FunctionExpr,
        _def_id: GlobalDefId,
        field: &FunctionFieldInit,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let union_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(union_ty, "uniontmp")
            .map_err(|_| self.error(expr.span, "failed to allocate union literal"))?;
        self.emit_union_literal_into(expr, field, ptr)?;
        self.builder
            .build_load(union_ty, ptr, "unionlit")
            .map_err(|_| self.error(expr.span, "failed to load union literal"))
    }

    pub(super) fn emit_union_literal_into(
        &mut self,
        _expr: &FunctionExpr,
        field: &FunctionFieldInit,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let value = self.emit_expr(&field.value)?;
        self.builder
            .build_store(ptr, value)
            .map_err(|_| self.error(field.span, "failed to store union field"))?;
        Ok(())
    }

    pub(super) fn emit_optional_null(
        &mut self,
        expr: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.emit_tagged_union(expr, FunctionOptionalTag::Null.discriminant(), None)
    }

    pub(super) fn emit_optional_some(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.emit_tagged_union(expr, FunctionOptionalTag::Some.discriminant(), Some(value))
    }

    pub(super) fn emit_error_union_value(
        &mut self,
        expr: &FunctionExpr,
        tag: FunctionErrorUnionTag,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.emit_tagged_union(expr, tag.discriminant(), Some(value))
    }

    fn emit_tagged_union(
        &mut self,
        expr: &FunctionExpr,
        tag: u8,
        payload: Option<&FunctionExpr>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(ty, "taggedtmp")
            .map_err(|_| self.error(expr.span, "failed to allocate tagged union literal"))?;
        self.emit_tagged_union_into(expr, tag, payload, ptr)?;
        self.builder
            .build_load(ty, ptr, "taggedlit")
            .map_err(|_| self.error(expr.span, "failed to load tagged union literal"))
    }

    pub(super) fn emit_tagged_union_into(
        &mut self,
        expr: &FunctionExpr,
        tag: u8,
        payload: Option<&FunctionExpr>,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let tag_ptr = self
            .builder
            .build_struct_gep(ty, ptr, 0, "tagptr")
            .map_err(|_| self.error(expr.span, "failed to build tagged union tag address"))?;
        let tag_value = self.module.context.i8_type().const_int(tag.into(), false);
        self.builder
            .build_store(tag_ptr, tag_value)
            .map_err(|_| self.error(expr.span, "failed to store tagged union tag"))?;
        if let Some(payload) = payload
            && !self.is_zero_sized(payload.ty)
        {
            let storage_ptr = self
                .builder
                .build_struct_gep(ty, ptr, 1, "payloadptr")
                .map_err(|_| {
                    self.error(expr.span, "failed to build tagged union payload address")
                })?;
            let value = self.emit_expr(payload)?;
            self.builder
                .build_store(storage_ptr, value)
                .map_err(|_| self.error(payload.span, "failed to store tagged union payload"))?;
        }
        Ok(())
    }

    pub(super) fn emit_enum_variant(
        &self,
        expr: &FunctionExpr,
        def_id: GlobalDefId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(variant_info) = self.module.program.enum_variant_infos.get(&def_id) else {
            return Err(self.error(expr.span, "missing enum variant"));
        };
        let enum_item = variant_info.owner;
        let value = variant_info
            .variant
            .value
            .unwrap_or(variant_info.index as i128);
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(enum_item.backing_type) else {
            return Err(self.error(expr.span, "enum backing type is not primitive"));
        };
        let ty = self.integer_llvm_type(*primitive, expr.span)?;
        Ok(ty.const_u128(value as u128).into())
    }

    fn emit_const_index_addr(
        &self,
        span: Span,
        array_ty: InternedTyId,
        base_ptr: PointerValue<'ctx>,
        index: u64,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let array_ty = self.module.llvm_basic_type(array_ty, span)?;
        let zero = self.module.context.i64_type().const_int(0, false);
        let index = self.module.context.i64_type().const_int(index, false);
        unsafe {
            self.builder
                .build_gep(array_ty, base_ptr, &[zero, index], "elemptr")
                .map_err(|_| self.error(span, "failed to build array element address"))
        }
    }
}
