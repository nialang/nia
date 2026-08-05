// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionArrayElements, FunctionBuiltinValue, FunctionErrorUnionTag, FunctionExpr,
    FunctionExprKind, FunctionFieldInit, FunctionOptionalTag, FunctionUnionRelocation,
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
                let count = self.module.array_len(count, expr.span)?;
                for index in 0..count {
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
            if self.is_zero_sized(self.field_ty(expr.ty, field_id, field.span)?) {
                self.emit_effect_expr(&field.value)?;
                continue;
            }
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

    pub(super) fn emit_union_storage_literal(
        &mut self,
        expr: &FunctionExpr,
        bytes: &[Option<u8>],
        relocations: &[FunctionUnionRelocation],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let union_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(union_ty, "unionstoragetmp")
            .map_err(|_| self.error(expr.span, "failed to allocate union storage literal"))?;
        self.emit_union_storage_literal_into(expr, bytes, relocations, ptr)?;
        self.builder
            .build_load(union_ty, ptr, "unionstorage")
            .map_err(|_| self.error(expr.span, "failed to load union storage literal"))
    }

    pub(super) fn emit_union_storage_literal_into(
        &mut self,
        expr: &FunctionExpr,
        bytes: &[Option<u8>],
        relocations: &[FunctionUnionRelocation],
        ptr: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let expected_size = self
            .module
            .layout_of(expr.ty)
            .and_then(|layout| usize::try_from(layout.size).ok());
        if expected_size != Some(bytes.len()) {
            return Err(self.error(expr.span, "union storage literal has the wrong byte length"));
        }
        let mut relocated_bytes = vec![false; bytes.len()];
        for relocation in relocations {
            let end = relocation
                .offset
                .checked_add(relocation.width)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| {
                    self.error(expr.span, "union storage relocation is out of bounds")
                })?;
            relocated_bytes[relocation.offset..end].fill(true);
        }
        let byte_ty = self.module.context.i8_type();
        for (offset, byte) in bytes.iter().enumerate() {
            if relocated_bytes[offset] {
                continue;
            }
            let Some(byte) = byte else {
                continue;
            };
            let offset = u64::try_from(offset)
                .map_err(|_| self.error(expr.span, "union storage offset is not representable"))?;
            let offset = self.module.context.i64_type().const_int(offset, false);
            let byte_ptr = unsafe {
                self.builder
                    .build_gep(byte_ty, ptr, &[offset], "union.byte.ptr")
                    .map_err(|_| self.error(expr.span, "failed to address union storage byte"))?
            };
            self.builder
                .build_store(byte_ptr, byte_ty.const_int(u64::from(*byte), false))
                .map_err(|_| self.error(expr.span, "failed to store union storage byte"))?;
        }
        for relocation in relocations {
            let offset = u64::try_from(relocation.offset).map_err(|_| {
                self.error(expr.span, "union relocation offset is not representable")
            })?;
            let offset = self.module.context.i64_type().const_int(offset, false);
            let relocation_ptr = unsafe {
                self.builder
                    .build_gep(byte_ty, ptr, &[offset], "union.relocation.ptr")
                    .map_err(|_| {
                        self.error(expr.span, "failed to address union relocation storage")
                    })?
            };
            let promoted = self.emit_promoted_allocation_pointer(relocation)?;
            let store = self
                .builder
                .build_store(relocation_ptr, promoted)
                .map_err(|_| self.error(expr.span, "failed to store union relocation pointer"))?;
            let align = u32::try_from(self.module.source.layouts.target.pointer_align)
                .map_err(|_| self.error(expr.span, "artifact pointer alignment is too large"))?;
            store.set_alignment(align);
        }
        Ok(())
    }

    fn emit_promoted_allocation_pointer(
        &mut self,
        relocation: &FunctionUnionRelocation,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let pointee = &relocation.pointee;
        let value = self.emit_promoted_const_value(pointee)?;
        self.module.materialize_promoted_allocation(
            relocation.allocation,
            pointee.ty,
            value,
            pointee.span,
        )
    }

    fn emit_promoted_const_value(
        &mut self,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match &value.kind {
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(integer)) => {
                let ty = self
                    .module
                    .llvm_basic_type(value.ty, value.span)?
                    .into_int_type()?;
                Ok(ty.const_u128(integer.bits()).into())
            }
            FunctionExprKind::Integer(text) => {
                self.emit_integer_literal(value.ty, value.span, text)
            }
            FunctionExprKind::Float(text) => self.emit_float_literal(value.ty, value.span, text),
            FunctionExprKind::Char(character) => {
                self.emit_char_literal(value.ty, value.span, *character)
            }
            FunctionExprKind::ByteChar(text) => {
                self.emit_byte_char_literal(value.ty, value.span, text)
            }
            FunctionExprKind::Bool(boolean) => Ok(self
                .module
                .context
                .bool_type()
                .const_int(u64::from(*boolean), false)
                .into()),
            FunctionExprKind::Null => Ok(self
                .module
                .llvm_basic_type(value.ty, value.span)?
                .into_pointer_type()?
                .const_null()
                .into()),
            FunctionExprKind::String(scalars) => {
                self.emit_string_literal(value.ty, value.span, scalars)
            }
            FunctionExprKind::ByteString(bytes) => {
                self.emit_byte_string_literal(value.ty, value.span, bytes)
            }
            FunctionExprKind::ArrayLiteral { elems } => {
                self.emit_promoted_array_const(value, elems)
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                self.emit_promoted_struct_const(value, fields)
            }
            _ => Err(self.error(
                value.span,
                "promoted allocation pointee is not yet an LLVM constant",
            )),
        }
    }

    fn emit_promoted_array_const(
        &mut self,
        expr: &FunctionExpr,
        elems: &FunctionArrayElements,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = self.module.ty_kind(expr.ty).cloned() else {
            return Err(self.error(expr.span, "promoted array initializer has a non-array type"));
        };
        let values = match elems {
            FunctionArrayElements::List(values) => values
                .iter()
                .map(|value| self.emit_promoted_const_value(value))
                .collect::<Result<Vec<_>, _>>()?,
            FunctionArrayElements::Repeat { value, count } => {
                let count = self.module.array_len(count, expr.span)?;
                let count = usize::try_from(count)
                    .map_err(|_| self.error(expr.span, "promoted array length is too large"))?;
                let value = self.emit_promoted_const_value(value)?;
                std::iter::repeat_n(value, count).collect()
            }
        };
        self.module
            .const_array_from_values_in_current(elem, &values, expr.span)
    }

    fn emit_promoted_struct_const(
        &mut self,
        expr: &FunctionExpr,
        fields: &[FunctionFieldInit],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.module.ty_kind(expr.ty).cloned()
        else {
            return Err(self.error(expr.span, "promoted struct initializer is not nominal"));
        };
        if self.module.is_union_def(def_id) {
            return Err(self.error(
                expr.span,
                "promoted union pointee requires relocation-aware constant storage",
            ));
        }
        let physical_fields = self
            .module
            .physical_struct_fields(def_id, &args, &const_args, expr.span)?
            .into_iter()
            .map(|field| field.def_id)
            .collect::<Vec<_>>();
        let mut values = Vec::with_capacity(physical_fields.len());
        for field_id in physical_fields {
            let field = fields
                .iter()
                .find(|field| field.field == Some(field_id))
                .ok_or_else(|| self.error(expr.span, "promoted struct field is missing"))?;
            values.push(self.emit_promoted_const_value(&field.value)?);
        }
        let struct_ty = self
            .module
            .llvm_basic_type(expr.ty, expr.span)?
            .into_struct_type()?;
        Ok(struct_ty.const_named_struct(&values).into())
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
        if let Some(payload) = payload {
            if self.is_zero_sized(payload.ty) {
                self.emit_effect_expr(payload)?;
                return Ok(());
            }
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
        &mut self,
        expr: &FunctionExpr,
        def_id: GlobalDefId,
        fields: &[FunctionExpr],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(variant_info) = self.module.program.enum_variant_info(def_id) else {
            return Err(self.error(expr.span, "missing enum variant"));
        };
        let enum_item = variant_info.owner;
        let Some(layout) = self.module.program.enum_layout(enum_item.def_id) else {
            return Err(self.error(expr.span, "missing enum layout"));
        };
        if layout.payload_offset.is_none() {
            return self.enum_variant_tag_value(expr.span, def_id, enum_item.backing_type);
        }
        let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(ty, "enumtmp")
            .map_err(|_| self.error(expr.span, "failed to allocate enum literal"))?;
        let tag_ptr = self
            .builder
            .build_struct_gep(ty, ptr, 0, "enum.tag.ptr")
            .map_err(|_| self.error(expr.span, "failed to address enum tag"))?;
        let tag = self.enum_variant_tag_value(expr.span, def_id, enum_item.backing_type)?;
        self.builder
            .build_store(tag_ptr, tag)
            .map_err(|_| self.error(expr.span, "failed to store enum tag"))?;
        let variant_layout = layout
            .variants
            .iter()
            .find(|variant| variant.def_id == def_id.def_id)
            .ok_or_else(|| self.error(expr.span, "missing enum variant layout"))?;
        let payload_offset = layout.payload_offset.unwrap_or(0);
        for (index, field) in fields.iter().enumerate() {
            let Some(field_layout) = variant_layout.fields.get(index) else {
                return Err(self.error(field.span, "missing enum payload field layout"));
            };
            let field_ptr = self.enum_byte_offset_ptr(
                ptr,
                payload_offset.saturating_add(field_layout.offset),
                field.span,
            )?;
            let value = self.emit_expr(field)?;
            self.builder
                .build_store(field_ptr, value)
                .map_err(|_| self.error(field.span, "failed to store enum payload field"))?;
        }
        self.builder
            .build_load(ty, ptr, "enumlit")
            .map_err(|_| self.error(expr.span, "failed to load enum literal"))
    }

    pub(super) fn emit_enum_variant_tag(
        &self,
        expr: &FunctionExpr,
        def_id: GlobalDefId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.enum_variant_tag_value(expr.span, def_id, expr.ty)
    }

    fn enum_variant_tag_value(
        &self,
        span: Span,
        def_id: GlobalDefId,
        backing_type: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(variant_info) = self.module.program.enum_variant_info(def_id) else {
            return Err(self.error(span, "missing enum variant"));
        };
        let value = variant_info
            .variant
            .value
            .unwrap_or(variant_info.index as i128);
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(backing_type) else {
            return Err(self.error(span, "enum backing type is not primitive"));
        };
        let ty = self.integer_llvm_type(*primitive, span)?;
        Ok(ty.const_u128(value as u128).into())
    }

    pub(super) fn emit_enum_tag(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = self.emit_expr(value)?;
        self.emit_enum_tag_from_value(expr, value)
    }

    fn emit_enum_tag_from_value(
        &self,
        expr: &FunctionExpr,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if value.is_int_value() {
            return Ok(value);
        }
        let aggregate = value
            .into_struct_value()
            .map_err(|_| self.error(expr.span, "enum value is not an aggregate"))?;
        self.builder
            .build_extract_value(aggregate, 0, "enum.tag")
            .map_err(|_| self.error(expr.span, "failed to extract enum tag"))
    }

    pub(super) fn emit_enum_payload_field(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
        variant: GlobalDefId,
        field: usize,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(variant_info) = self.module.program.enum_variant_info(variant) else {
            return Err(self.error(expr.span, "missing enum variant"));
        };
        let Some(layout) = self.module.program.enum_layout(variant_info.owner.def_id) else {
            return Err(self.error(expr.span, "missing enum layout"));
        };
        let payload_offset = layout
            .payload_offset
            .ok_or_else(|| self.error(expr.span, "enum variant has no payload storage"))?;
        let variant_layout = layout
            .variants
            .iter()
            .find(|candidate| candidate.def_id == variant.def_id)
            .ok_or_else(|| self.error(expr.span, "missing enum variant layout"))?;
        let field_layout = variant_layout
            .fields
            .get(field)
            .ok_or_else(|| self.error(expr.span, "missing enum payload field layout"))?;
        let enum_ty = self.module.llvm_basic_type(value.ty, value.span)?;
        let ptr = self
            .builder
            .build_alloca(enum_ty, "enum.payload.copy")
            .map_err(|_| self.error(expr.span, "failed to allocate enum payload copy"))?;
        let value = self.emit_expr(value)?;
        self.builder
            .build_store(ptr, value)
            .map_err(|_| self.error(expr.span, "failed to store enum payload copy"))?;
        let field_ptr = self.enum_byte_offset_ptr(
            ptr,
            payload_offset.saturating_add(field_layout.offset),
            expr.span,
        )?;
        let field_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        self.builder
            .build_load(field_ty, field_ptr, "enum.payload.field")
            .map_err(|_| self.error(expr.span, "failed to load enum payload field"))
    }

    fn enum_byte_offset_ptr(
        &self,
        ptr: PointerValue<'ctx>,
        offset: u64,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let offset = self.module.context.i64_type().const_int(offset, false);
        unsafe {
            self.builder
                .build_gep(
                    self.module.context.i8_type(),
                    ptr,
                    &[offset],
                    "enum.byte.ptr",
                )
                .map_err(|_| self.error(span, "failed to address enum payload"))
        }
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
