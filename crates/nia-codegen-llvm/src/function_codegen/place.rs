// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::AssignOp;
use nia_backend_ir::{TypedExpr, TypedExprKind, TypedPlace, TypedSliceRange};
use nia_diagnostic::Diagnostic;
use nia_ids::{InternedTyId, LocalId};
use nia_llvm::{
    types::BasicTypeEnum,
    values::{BasicValueEnum, IntValue, PointerValue, StructValue},
};
use nia_span::Span;
use nia_ty::TyKind;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_addr_of(
        &mut self,
        expr: &TypedExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &expr.kind {
            TypedExprKind::Global(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | TypedExprKind::Field { .. }
            | TypedExprKind::Index { .. } => self.emit_place_addr(expr.span, expr),
            TypedExprKind::CStringPointer { array, .. } => self
                .emit_c_string_pointer(expr.span, array)
                .map(|value| value.into_pointer_value().expect("C string pointer value")),
            _ => Err(self.error(expr.span, "expression is not a place")),
        }
    }

    fn emit_place_addr(
        &mut self,
        span: Span,
        expr: &TypedExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &expr.kind {
            TypedExprKind::Global(def_id) => self
                .module
                .globals
                .get(def_id)
                .map(|global| global.as_pointer_value())
                .ok_or_else(|| self.error(span, "missing global value")),
            TypedExprKind::Local(local_id) => self
                .locals
                .get(local_id)
                .copied()
                .ok_or_else(|| self.error(span, "missing local storage")),
            TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => Ok(self.emit_expr(expr)?.into_pointer_value()?),
            TypedExprKind::Field { lhs, field } => {
                let (base_ty, base_ptr) = self.emit_struct_base_addr(lhs)?;
                if let Some((def_id, _)) = self.module_field_base_type(lhs.ty)
                    && self.module.is_union_def(def_id)
                {
                    return Ok(base_ptr);
                }
                let field_index = self.field_index(lhs.ty, *field, span)?;
                self.builder
                    .build_struct_gep(base_ty, base_ptr, field_index, "fieldptr")
                    .map_err(|_| self.error(span, "failed to build field address"))
            }
            TypedExprKind::Index { lhs, index } => self.emit_index_expr_addr(span, lhs, index),
            _ => Err(self.error(span, "expression is not a place")),
        }
    }

    fn emit_array_temp_addr(&mut self, expr: &TypedExpr) -> Result<PointerValue<'ctx>, Diagnostic> {
        let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self
            .builder
            .build_alloca(ty, "arraytmp")
            .map_err(|_| self.error(expr.span, "failed to allocate slice temporary"))?;
        let value = self.emit_expr(expr)?;
        self.builder
            .build_store(ptr, value)
            .map_err(|_| self.error(expr.span, "failed to store slice temporary"))?;
        Ok(ptr)
    }

    fn module_field_base_type(
        &self,
        ty: InternedTyId,
    ) -> Option<(nia_ids::GlobalDefId, Vec<InternedTyId>)> {
        match self.module.ty_kind(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.module_field_base_type(*elem),
            _ => None,
        }
    }

    pub(super) fn emit_slice(
        &mut self,
        span: Span,
        lhs: &TypedExpr,
        range: &TypedSliceRange,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let (base_ptr, base_len, elem_ty) = self.emit_slice_base(span, lhs)?;
        let start = self.emit_range_start(range)?;
        let end = self.emit_range_end(range, base_len)?;
        let len = self
            .builder
            .build_int_sub(end, start, "slicelen")
            .map_err(|_| self.error(span, "failed to compute slice length"))?;
        let elem_llvm_ty = self.module.llvm_basic_type(elem_ty, span)?;
        let ptr = unsafe {
            self.builder
                .build_gep(elem_llvm_ty, base_ptr, &[start], "sliceptr")
                .map_err(|_| self.error(span, "failed to build slice pointer"))?
        };
        self.build_slice_value(ptr, len)
    }

    fn emit_index_expr_addr(
        &mut self,
        span: Span,
        lhs: &TypedExpr,
        index: &TypedExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let index_value = self.emit_usize_value(index)?;
        match self.module.ty_kind(lhs.ty) {
            Some(TyKind::Array { .. }) => {
                let base_ptr = self.emit_addr_of(lhs)?;
                let array_ty = self.module.llvm_basic_type(lhs.ty, span)?;
                let zero = self.module.context.i64_type().const_int(0, false);
                unsafe {
                    self.builder
                        .build_gep(array_ty, base_ptr, &[zero, index_value], "elemptr")
                        .map_err(|_| self.error(span, "failed to build array element address"))
                }
            }
            Some(TyKind::Pointer { elem, .. }) => {
                let base_ptr = self.emit_expr(lhs)?.into_pointer_value()?;
                let elem_ty = self.module.llvm_basic_type(*elem, span)?;
                unsafe {
                    self.builder
                        .build_gep(elem_ty, base_ptr, &[index_value], "ptrelem")
                        .map_err(|_| self.error(span, "failed to build pointer element address"))
                }
            }
            Some(TyKind::Slice { elem, .. }) => {
                let slice = self.emit_expr(lhs)?.into_struct_value()?;
                let base_ptr = self.extract_slice_ptr(span, slice)?;
                let elem_ty = self.module.llvm_basic_type(*elem, span)?;
                unsafe {
                    self.builder
                        .build_gep(elem_ty, base_ptr, &[index_value], "sliceelem")
                        .map_err(|_| self.error(span, "failed to build slice element address"))
                }
            }
            _ => Err(self.error(span, "index base must be an array, pointer, or slice")),
        }
    }

    fn emit_slice_base(
        &mut self,
        span: Span,
        lhs: &TypedExpr,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>, InternedTyId), Diagnostic> {
        let one = self.module.context.i64_type().const_int(1, false);
        match self.module.ty_kind(lhs.ty) {
            Some(TyKind::Array { len, elem }) => {
                let base_ptr = self.emit_array_base_addr(lhs)?;
                let array_len = self.module.array_len(len, span)?;
                let len = self.module.context.i64_type().const_int(array_len, false);
                let zero = self.module.context.i64_type().const_int(0, false);
                let array_ty = self.module.llvm_basic_type(lhs.ty, span)?;
                let ptr = unsafe {
                    self.builder
                        .build_gep(array_ty, base_ptr, &[zero, zero], "arraydecay")
                        .map_err(|_| self.error(span, "failed to build array slice base"))?
                };
                Ok((ptr, len, *elem))
            }
            Some(TyKind::Pointer { elem, .. }) => {
                let ptr = self.emit_expr(lhs)?.into_pointer_value()?;
                Ok((ptr, one, *elem))
            }
            Some(TyKind::Slice { elem, .. }) => {
                let slice = self.emit_expr(lhs)?.into_struct_value()?;
                let ptr = self.extract_slice_ptr(span, slice)?;
                let len = self.extract_slice_len(span, slice)?;
                Ok((ptr, len, *elem))
            }
            Some(TyKind::Error) | None => Err(self.error(span, "invalid slice base")),
            _ => Err(self.error(span, "slice base must be an array, pointer, or slice")),
        }
    }

    fn emit_array_base_addr(&mut self, lhs: &TypedExpr) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &lhs.kind {
            TypedExprKind::Global(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | TypedExprKind::Field { .. }
            | TypedExprKind::Index { .. } => self.emit_place_addr(lhs.span, lhs),
            _ => self.emit_array_temp_addr(lhs),
        }
    }

    pub(super) fn emit_c_string_pointer(
        &mut self,
        span: Span,
        array: &TypedExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { .. }) = self.module.ty_kind(array.ty) else {
            return Err(self.error(span, "C string literal pointer source is not an array"));
        };
        let base_ptr = self.emit_array_temp_addr(array)?;
        let array_ty = self.module.llvm_basic_type(array.ty, span)?;
        let zero = self.module.context.i64_type().const_int(0, false);
        let ptr = unsafe {
            self.builder
                .build_gep(array_ty, base_ptr, &[zero, zero], "cstr")
                .map_err(|_| self.error(span, "failed to build C string literal pointer"))?
        };
        Ok(ptr.into())
    }

    fn emit_range_start(&mut self, range: &TypedSliceRange) -> Result<IntValue<'ctx>, Diagnostic> {
        if let Some(start) = &range.start {
            self.emit_usize_value(start)
        } else {
            Ok(self.module.context.i64_type().const_int(0, false))
        }
    }

    fn emit_range_end(
        &mut self,
        range: &TypedSliceRange,
        base_len: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        let end = if let Some(end) = &range.end {
            self.emit_usize_value(end)?
        } else {
            base_len
        };
        if range.inclusive {
            self.builder
                .build_int_add(
                    end,
                    self.module.context.i64_type().const_int(1, false),
                    "sliceend",
                )
                .map_err(|_| self.error(Span::default(), "failed to compute inclusive slice end"))
        } else {
            Ok(end)
        }
    }

    fn emit_usize_value(&mut self, expr: &TypedExpr) -> Result<IntValue<'ctx>, Diagnostic> {
        let value = self.emit_expr(expr)?.into_int_value()?;
        let target = self.module.context.i64_type();
        let bits = value.get_type().bit_width();
        if bits == 64 {
            Ok(value)
        } else if bits > 64 {
            self.builder
                .build_int_truncate(value, target, "usizecast")
                .map_err(|_| self.error(expr.span, "failed to truncate range bound"))
        } else {
            self.builder
                .build_int_z_extend(value, target, "usizecast")
                .map_err(|_| self.error(expr.span, "failed to extend range bound"))
        }
    }

    fn build_slice_value(
        &self,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let undef = self.module.slice_type().get_undef();
        let value = self
            .builder
            .build_insert_value(undef, ptr, 0, "slice.ptr")
            .map_err(|_| self.error(Span::default(), "failed to insert slice pointer"))?
            .into_struct_value()?;
        self.builder
            .build_insert_value(value, len, 1, "slice.len")
            .map_err(|_| self.error(Span::default(), "failed to insert slice length"))
    }

    fn extract_slice_ptr(
        &self,
        span: Span,
        slice: StructValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let value = self
            .builder
            .build_extract_value(slice, 0, "slice.ptr")
            .map_err(|_| self.error(span, "failed to extract slice pointer"))?;
        Ok(value.into_pointer_value()?)
    }

    fn extract_slice_len(
        &self,
        span: Span,
        slice: StructValue<'ctx>,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        let value = self
            .builder
            .build_extract_value(slice, 1, "slice.len")
            .map_err(|_| self.error(span, "failed to extract slice length"))?;
        Ok(value.into_int_value()?)
    }

    pub(super) fn emit_len(
        &mut self,
        span: Span,
        inner: &TypedExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.module.ty_kind(inner.ty) {
            Some(TyKind::Array { len, .. }) => {
                let len = self.module.array_len(len, span)?;
                Ok(self.module.context.i64_type().const_int(len, false).into())
            }
            Some(TyKind::Slice { .. }) => {
                let slice = self.emit_expr(inner)?.into_struct_value()?;
                self.extract_slice_len(span, slice).map(Into::into)
            }
            _ => Err(self.error(span, "`@len` requires an array or slice")),
        }
    }

    pub(super) fn emit_ptr(
        &mut self,
        span: Span,
        inner: &TypedExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let slice = self.emit_expr(inner)?.into_struct_value()?;
        self.extract_slice_ptr(span, slice).map(Into::into)
    }

    pub(super) fn emit_assign(
        &mut self,
        span: Span,
        place: &nia_backend_ir::TypedPlace,
        op: AssignOp,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        let nia_backend_ir::PlaceBase::Local(LocalId(id)) = place.base else {
            let ptr = self.emit_typed_place_addr(place)?;
            let stored = if op == AssignOp::Assign {
                value
            } else {
                let current = self
                    .builder
                    .build_load(
                        self.module.llvm_basic_type(place.ty, place.span)?,
                        ptr,
                        "loadtmp",
                    )
                    .map_err(|_| self.error(span, "failed to load assignment target"))?;
                self.emit_compound_assignment(span, place.ty, current, op, value)?
            };
            self.builder
                .build_store(ptr, stored)
                .map_err(|_| self.error(span, "failed to store assignment"))?;
            return Ok(());
        };
        if id == u32::MAX {
            return Ok(());
        }
        let ptr = self.emit_typed_place_addr(place)?;
        let stored = if op == AssignOp::Assign {
            value
        } else {
            let current = self
                .builder
                .build_load(
                    self.module.llvm_basic_type(place.ty, place.span)?,
                    ptr,
                    "loadtmp",
                )
                .map_err(|_| self.error(span, "failed to load assignment target"))?;
            self.emit_compound_assignment(span, place.ty, current, op, value)?
        };
        self.builder
            .build_store(ptr, stored)
            .map_err(|_| self.error(span, "failed to store assignment"))?;
        Ok(())
    }

    pub(super) fn emit_typed_place_addr(
        &mut self,
        place: &TypedPlace,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let mut ptr = match &place.base {
            nia_backend_ir::PlaceBase::Local(local_id) => self
                .locals
                .get(local_id)
                .copied()
                .ok_or_else(|| self.error(place.span, "missing local storage"))?,
            nia_backend_ir::PlaceBase::Global(def_id) => self
                .module
                .globals
                .get(def_id)
                .map(|global| global.as_pointer_value())
                .ok_or_else(|| self.error(place.span, "missing global value"))?,
            nia_backend_ir::PlaceBase::Deref(expr) => self.emit_expr(expr)?.into_pointer_value()?,
        };
        let mut current_ty = self.place_base_ty(place);
        for elem in &place.elems {
            match elem {
                nia_backend_ir::PlaceElem::Field(field) => {
                    if let Some(TyKind::Pointer { elem, .. }) = self.module.ty_kind(current_ty) {
                        let ptr_ty = self.module.llvm_basic_type(current_ty, place.span)?;
                        ptr = self
                            .builder
                            .build_load(ptr_ty, ptr, "autoderef")
                            .map_err(|_| {
                                self.error(place.span, "failed to load pointer for field access")
                            })?
                            .into_pointer_value()?;
                        current_ty = *elem;
                    }
                    let base_ty = self.module.llvm_basic_type(current_ty, place.span)?;
                    if !self.is_union_ty(current_ty) {
                        let field_index = self.field_index(current_ty, *field, place.span)?;
                        ptr = self
                            .builder
                            .build_struct_gep(base_ty, ptr, field_index, "fieldptr")
                            .map_err(|_| self.error(place.span, "failed to build field address"))?;
                    }
                    current_ty = self.field_ty(current_ty, *field, place.span)?;
                }
                nia_backend_ir::PlaceElem::Index(index) => {
                    ptr = self.emit_index_addr(place.span, current_ty, ptr, index)?;
                    current_ty = self.array_elem_ty(current_ty, place.span)?;
                }
            }
        }
        Ok(ptr)
    }

    fn emit_struct_base_addr(
        &mut self,
        expr: &TypedExpr,
    ) -> Result<(BasicTypeEnum<'ctx>, PointerValue<'ctx>), Diagnostic> {
        match self.module.ty_kind(expr.ty) {
            Some(TyKind::Pointer { elem, .. }) => {
                let ptr = self.emit_expr(expr)?.into_pointer_value()?;
                let ty = self.module.llvm_basic_type(*elem, expr.span)?;
                Ok((ty, ptr))
            }
            _ => {
                let ptr = self.emit_addr_of(expr)?;
                let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
                Ok((ty, ptr))
            }
        }
    }

    fn place_base_ty(&self, place: &TypedPlace) -> InternedTyId {
        match &place.base {
            nia_backend_ir::PlaceBase::Local(local_id) => {
                self.local_tys.get(local_id).copied().unwrap_or(place.ty)
            }
            nia_backend_ir::PlaceBase::Global(def_id) => self
                .module
                .program
                .globals
                .get(def_id)
                .map(|global| global.ty)
                .unwrap_or(place.ty),
            nia_backend_ir::PlaceBase::Deref(expr) => match self.module.ty_kind(expr.ty) {
                Some(TyKind::Pointer { elem, .. }) => *elem,
                _ => place.ty,
            },
        }
    }

    fn is_union_ty(&self, ty: InternedTyId) -> bool {
        self.module_field_base_type(ty)
            .is_some_and(|(def_id, _)| self.module.is_union_def(def_id))
    }

    fn emit_index_addr(
        &mut self,
        span: Span,
        base_ty: InternedTyId,
        base_ptr: PointerValue<'ctx>,
        index: &TypedExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let index_value = self.emit_usize_value(index)?;
        match self.module.ty_kind(base_ty) {
            Some(TyKind::Array { .. }) => {
                let array_ty = self.module.llvm_basic_type(base_ty, span)?;
                let zero = self.module.context.i64_type().const_int(0, false);
                unsafe {
                    self.builder
                        .build_gep(array_ty, base_ptr, &[zero, index_value], "elemptr")
                        .map_err(|_| self.error(span, "failed to build array element address"))
                }
            }
            Some(TyKind::Pointer { elem, .. }) => {
                let ptr_ty = self.module.llvm_basic_type(base_ty, span)?;
                let ptr = self
                    .builder
                    .build_load(ptr_ty, base_ptr, "loadptr")
                    .map_err(|_| self.error(span, "failed to load pointer base"))?
                    .into_pointer_value()?;
                let elem_ty = self.module.llvm_basic_type(*elem, span)?;
                unsafe {
                    self.builder
                        .build_gep(elem_ty, ptr, &[index_value], "ptrelem")
                        .map_err(|_| self.error(span, "failed to build pointer element address"))
                }
            }
            Some(TyKind::Slice { elem, .. }) => {
                let slice_ty = self.module.llvm_basic_type(base_ty, span)?;
                let slice = self
                    .builder
                    .build_load(slice_ty, base_ptr, "loadslice")
                    .map_err(|_| self.error(span, "failed to load slice base"))?
                    .into_struct_value()?;
                let ptr = self.extract_slice_ptr(span, slice)?;
                let elem_ty = self.module.llvm_basic_type(*elem, span)?;
                unsafe {
                    self.builder
                        .build_gep(elem_ty, ptr, &[index_value], "sliceelem")
                        .map_err(|_| self.error(span, "failed to build slice element address"))
                }
            }
            _ => Err(self.error(span, "index base must be an array, pointer, or slice")),
        }
    }
}
