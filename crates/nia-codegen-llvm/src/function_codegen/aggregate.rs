// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{TypedArrayElements, TypedExpr, TypedFieldInit};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_llvm::values::{BasicValueEnum, PointerValue};
use nia_span::Span;
use nia_ty::TyKind;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_array_literal(
        &mut self,
        expr: &TypedExpr,
        elems: &TypedArrayElements,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let array_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(array_ty, "arraytmp")
            .map_err(|_| self.error(expr.span, "failed to allocate array literal"))?;
        match elems {
            TypedArrayElements::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    let elem_ptr =
                        self.emit_const_index_addr(expr.span, expr.ty, ptr, index as u64)?;
                    let value = self.emit_expr(value)?;
                    self.builder
                        .build_store(elem_ptr, value)
                        .map_err(|_| self.error(expr.span, "failed to store array element"))?;
                }
            }
            TypedArrayElements::Repeat { value, count } => {
                for index in 0..*count {
                    let elem_ptr = self.emit_const_index_addr(expr.span, expr.ty, ptr, index)?;
                    let value = self.emit_expr(value)?;
                    self.builder.build_store(elem_ptr, value).map_err(|_| {
                        self.error(expr.span, "failed to store repeated array element")
                    })?;
                }
            }
        }
        self.builder
            .build_load(array_ty, ptr, "arraylit")
            .map_err(|_| self.error(expr.span, "failed to load array literal"))
    }

    pub(super) fn emit_struct_literal(
        &mut self,
        expr: &TypedExpr,
        _def_id: GlobalDefId,
        fields: &[TypedFieldInit],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let struct_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(struct_ty, "structtmp")
            .map_err(|_| self.error(expr.span, "failed to allocate struct literal"))?;
        for field in fields {
            let field_index = self.field_index(expr.ty, field.field, field.span)?;
            let field_ptr = self
                .builder
                .build_struct_gep(struct_ty, ptr, field_index, "fieldptr")
                .map_err(|_| self.error(field.span, "failed to build struct field address"))?;
            let value = self.emit_expr(&field.value)?;
            self.builder
                .build_store(field_ptr, value)
                .map_err(|_| self.error(field.span, "failed to store struct field"))?;
        }
        self.builder
            .build_load(struct_ty, ptr, "structlit")
            .map_err(|_| self.error(expr.span, "failed to load struct literal"))
    }

    pub(super) fn emit_union_literal(
        &mut self,
        expr: &TypedExpr,
        _def_id: GlobalDefId,
        field: &TypedFieldInit,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let union_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(union_ty, "uniontmp")
            .map_err(|_| self.error(expr.span, "failed to allocate union literal"))?;
        let value = self.emit_expr(&field.value)?;
        self.builder
            .build_store(ptr, value)
            .map_err(|_| self.error(field.span, "failed to store union field"))?;
        self.builder
            .build_load(union_ty, ptr, "unionlit")
            .map_err(|_| self.error(expr.span, "failed to load union literal"))
    }

    pub(super) fn emit_enum_variant(
        &self,
        expr: &TypedExpr,
        def_id: GlobalDefId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(enum_item) = self
            .module
            .program
            .enums
            .values()
            .find(|item| item.variants.iter().any(|variant| variant.def_id == def_id))
        else {
            return Err(self.error(expr.span, "missing enum variant"));
        };
        let Some(variant_index) = enum_item
            .variants
            .iter()
            .position(|variant| variant.def_id == def_id)
        else {
            return Err(self.error(expr.span, "missing enum variant"));
        };
        let value = enum_item
            .variants
            .iter()
            .find(|variant| variant.def_id == def_id)
            .and_then(|variant| variant.value)
            .unwrap_or(variant_index as i128);
        let Some(TyKind::Primitive(primitive)) = self.module.interner().get(enum_item.backing_type)
        else {
            return Err(self.error(expr.span, "enum backing type is not primitive"));
        };
        let ty = self.integer_llvm_type(*primitive, expr.span)?;
        Ok(ty.const_u128(value as u128).into())
    }

    fn emit_const_index_addr(
        &self,
        span: Span,
        array_ty: TyId,
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
