// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::AssignOp;
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionExpr, FunctionExprKind, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionSliceRange,
};
use nia_ids::InternedTyId;
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
        expr: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &expr.kind {
            FunctionExprKind::AddrOf(place) => self.emit_typed_place_addr(place),
            FunctionExprKind::Global(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | FunctionExprKind::Field { .. }
            | FunctionExprKind::Index { .. } => self.emit_place_addr(expr.span, expr),
            FunctionExprKind::StaticArrayPointer { array, .. } => self
                .emit_static_array_pointer(expr.span, array)?
                .into_pointer_value()
                .map_err(|_| self.error(expr.span, "static array pointer value is not a pointer")),
            _ => Err(self.error(expr.span, "expression is not a place")),
        }
    }

    fn emit_place_addr(
        &mut self,
        span: Span,
        expr: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &expr.kind {
            FunctionExprKind::Global(def_id) => self
                .module
                .globals
                .get(def_id)
                .map(|global| global.as_pointer_value())
                .ok_or_else(|| self.error(span, "missing global value")),
            FunctionExprKind::Local(local_id) => self.local_addr(*local_id, span),
            FunctionExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => Ok(self.emit_expr(expr)?.into_pointer_value()?),
            FunctionExprKind::Field { lhs, field } => {
                let (base_ty, base_ptr) = self.emit_struct_base_addr(lhs)?;
                if let Some((def_id, _)) = self.module_field_base_type(lhs.ty)
                    && self.module.is_union_def(def_id)
                {
                    return Ok(base_ptr);
                }
                if self.is_zero_sized_field(lhs.ty, *field, span)? {
                    return Ok(base_ptr);
                }
                let field_index = self.field_index(lhs.ty, *field, span)?;
                self.builder
                    .build_struct_gep(base_ty, base_ptr, field_index, "fieldptr")
                    .map_err(|_| self.error(span, "failed to build field address"))
            }
            FunctionExprKind::Index { lhs, index } => self.emit_index_expr_addr(span, lhs, index),
            FunctionExprKind::AddrOf(place) => self.emit_typed_place_addr(place),
            _ => Err(self.error(span, "expression is not a place")),
        }
    }

    fn emit_array_temp_addr(
        &mut self,
        expr: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
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
            Some(TyKind::Nominal { def_id, args, .. }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.module_field_base_type(*elem),
            _ => None,
        }
    }

    pub(super) fn emit_slice(
        &mut self,
        span: Span,
        lhs: &FunctionExpr,
        range: &FunctionSliceRange,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let (base_ptr, base_len, elem_ty) = self.emit_slice_base(span, lhs)?;
        let start = self.emit_range_start(range)?;
        let end = self.emit_range_end(range, base_len)?;
        let len = self
            .builder
            .build_int_sub(end, start, "slicelen")
            .map_err(|_| self.error(span, "failed to compute slice length"))?;
        let ptr = self.emit_elem_offset_ptr(span, elem_ty, base_ptr, start, "sliceptr")?;
        self.build_slice_value(ptr, len)
    }

    fn emit_index_expr_addr(
        &mut self,
        span: Span,
        lhs: &FunctionExpr,
        index: &FunctionExpr,
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
                let slice = self
                    .emit_expr(lhs)?
                    .into_struct_value()
                    .map_err(|_| self.error(span, "index base slice value is not a struct"))?;
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
        lhs: &FunctionExpr,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>, InternedTyId), Diagnostic> {
        let one = self.module.context.i64_type().const_int(1, false);
        match self.module.ty_kind(lhs.ty) {
            Some(TyKind::Array { len, elem }) => {
                let base_ptr = self.emit_array_base_addr(lhs)?;
                let array_len = self.module.array_len(len, span)?;
                let len = self.module.context.i64_type().const_int(array_len, false);
                if self.is_zero_sized(lhs.ty) {
                    return Ok((base_ptr, len, *elem));
                }
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
                if let Some(TyKind::Array {
                    len,
                    elem: array_elem,
                }) = self.module.ty_kind(*elem)
                {
                    let array_len = self.module.array_len(len, span)?;
                    let len = self.module.context.i64_type().const_int(array_len, false);
                    if self.is_zero_sized(*elem) {
                        return Ok((ptr, len, *array_elem));
                    }
                    let zero = self.module.context.i64_type().const_int(0, false);
                    let array_ty = self.module.llvm_basic_type(*elem, span)?;
                    let elem_ptr = unsafe {
                        self.builder
                            .build_gep(array_ty, ptr, &[zero, zero], "arrayptrdecay")
                            .map_err(|_| {
                                self.error(span, "failed to build pointer array slice base")
                            })?
                    };
                    return Ok((elem_ptr, len, *array_elem));
                }
                Ok((ptr, one, *elem))
            }
            Some(TyKind::Slice { elem, .. }) => {
                let slice = self
                    .emit_expr(lhs)?
                    .into_struct_value()
                    .map_err(|_| self.error(span, "slice base value is not a struct"))?;
                let ptr = self.extract_slice_ptr(span, slice)?;
                let len = self.extract_slice_len(span, slice)?;
                Ok((ptr, len, *elem))
            }
            Some(TyKind::Error) => Err(self.error(span, "invalid slice base")),
            None => Err(self.error(
                span,
                "slice base type is missing from backend type interner",
            )),
            _ => Err(self.error(span, "slice base must be an array, pointer, or slice")),
        }
    }

    fn emit_elem_offset_ptr(
        &mut self,
        span: Span,
        elem_ty: InternedTyId,
        base_ptr: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        if self.is_zero_sized(elem_ty) {
            return Ok(base_ptr);
        }
        let elem_llvm_ty = self.module.llvm_basic_type(elem_ty, span)?;
        unsafe {
            self.builder
                .build_gep(elem_llvm_ty, base_ptr, &[index], name)
                .map_err(|_| self.error(span, "failed to build element pointer"))
        }
    }

    fn emit_array_base_addr(
        &mut self,
        lhs: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match &lhs.kind {
            FunctionExprKind::Global(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | FunctionExprKind::Field { .. }
            | FunctionExprKind::Index { .. } => self.emit_place_addr(lhs.span, lhs),
            _ => self.emit_array_temp_addr(lhs),
        }
    }

    pub(super) fn emit_static_array_pointer(
        &mut self,
        span: Span,
        array: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if let FunctionExprKind::StaticArrayPointer { array, .. } = &array.kind {
            return self.emit_static_array_pointer(span, array);
        }
        let Some(TyKind::Array { .. }) = self.module.ty_kind(array.ty) else {
            return Err(self.error(span, "string literal pointer source is not an array"));
        };
        let array_ty = self.module.llvm_basic_type(array.ty, span)?;
        let value = match &array.kind {
            FunctionExprKind::String(scalars) => {
                self.emit_string_literal(array.ty, array.span, scalars)?
            }
            FunctionExprKind::ByteString(bytes) => {
                self.emit_byte_string_literal(array.ty, array.span, bytes)?
            }
            _ => self.emit_expr(array)?,
        };
        let ptr = self
            .module
            .materialize_static_array_pointer(array_ty, value, span)?;
        Ok(ptr.into())
    }

    fn emit_range_start(
        &mut self,
        range: &FunctionSliceRange,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        if let Some(start) = &range.start {
            self.emit_usize_value(start)
        } else {
            Ok(self.module.context.i64_type().const_int(0, false))
        }
    }

    fn emit_range_end(
        &mut self,
        range: &FunctionSliceRange,
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

    fn emit_usize_value(&mut self, expr: &FunctionExpr) -> Result<IntValue<'ctx>, Diagnostic> {
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
            .into_struct_value()
            .map_err(|_| self.error(Span::default(), "slice value is not a struct"))?;
        self.builder
            .build_insert_value(value, len, 1, "slice.len")
            .map_err(|_| self.error(Span::default(), "failed to insert slice length"))
    }

    pub(super) fn extract_slice_ptr(
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

    pub(super) fn extract_slice_len(
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

    pub(super) fn emit_assign(
        &mut self,
        span: Span,
        place: &FunctionPlace,
        op: AssignOp,
        rhs_ty: InternedTyId,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        if matches!(place.base, FunctionPlaceBase::Error) {
            return Ok(());
        }
        let ptr = self.emit_typed_place_addr(place)?;
        let is_volatile = self.place_access_is_volatile(place);
        let stored = if op == AssignOp::Assign {
            value
        } else {
            let ty = self.module.llvm_basic_type(place.ty, place.span)?;
            let current = self
                .build_place_load(ty, ptr, "loadtmp", is_volatile)
                .map_err(|_| self.error(span, "failed to load assignment target"))?;
            self.emit_compound_assignment(span, place.ty, current, op, rhs_ty, value)?
        };
        self.build_place_store(ptr, stored, is_volatile)
            .map_err(|_| self.error(span, "failed to store assignment"))?;
        Ok(())
    }

    pub(super) fn emit_typed_place_addr(
        &mut self,
        place: &FunctionPlace,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let mut ptr = match &place.base {
            FunctionPlaceBase::Local(local_id) => self.local_addr(*local_id, place.span)?,
            FunctionPlaceBase::Global(def_id) => self
                .module
                .globals
                .get(def_id)
                .map(|global| global.as_pointer_value())
                .ok_or_else(|| self.error(place.span, "missing global value"))?,
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => self
                .module
                .global_instances
                .get(&(*def_id, *arg_module_id, args.clone(), const_args.clone()))
                .map(|global| global.as_pointer_value())
                .ok_or_else(|| self.error(place.span, "missing global instance value"))?,
            FunctionPlaceBase::Deref(expr) => self.emit_expr(expr)?.into_pointer_value()?,
            FunctionPlaceBase::Error => return Err(self.error(place.span, "invalid place")),
        };
        let mut current_ty = self.place_base_ty(place);
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(field) => {
                    if let Some(
                        TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. },
                    ) = self.module.ty_kind(current_ty)
                    {
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
                        if self.is_zero_sized_field(current_ty, *field, place.span)? {
                            current_ty = self.field_ty(current_ty, *field, place.span)?;
                            continue;
                        }
                        let field_index = self.field_index(current_ty, *field, place.span)?;
                        ptr = self
                            .builder
                            .build_struct_gep(base_ty, ptr, field_index, "fieldptr")
                            .map_err(|_| self.error(place.span, "failed to build field address"))?;
                    }
                    current_ty = self.field_ty(current_ty, *field, place.span)?;
                }
                FunctionPlaceElem::Index(index) => {
                    ptr = self.emit_index_addr(place.span, current_ty, ptr, index)?;
                    current_ty = self.array_elem_ty(current_ty, place.span)?;
                }
                FunctionPlaceElem::Error => return Err(self.error(place.span, "invalid place")),
            }
        }
        Ok(ptr)
    }

    fn is_zero_sized_field(
        &self,
        base_ty: InternedTyId,
        field: nia_ids::GlobalDefId,
        span: Span,
    ) -> Result<bool, Diagnostic> {
        let field_ty = self.field_ty(base_ty, field, span)?;
        Ok(self.is_zero_sized(field_ty))
    }

    fn emit_struct_base_addr(
        &mut self,
        expr: &FunctionExpr,
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

    fn place_base_ty(&self, place: &FunctionPlace) -> InternedTyId {
        match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                self.local_tys.get(local_id).copied().unwrap_or(place.ty)
            }
            FunctionPlaceBase::Global(def_id) => self
                .module
                .program
                .global(*def_id)
                .map(|global| global.ty)
                .unwrap_or(place.ty),
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => self
                .module
                .program
                .global_instance(*def_id, *arg_module_id, args, const_args)
                .map(|global| global.ty)
                .unwrap_or(place.ty),
            FunctionPlaceBase::Deref(expr) => match self.module.ty_kind(expr.ty) {
                Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => *elem,
                _ => place.ty,
            },
            FunctionPlaceBase::Error => place.ty,
        }
    }

    pub(super) fn place_access_is_volatile(&self, place: &FunctionPlace) -> bool {
        match &place.base {
            FunctionPlaceBase::Deref(expr) => matches!(
                self.module.ty_kind(expr.ty),
                Some(TyKind::VolatilePointer { .. })
            ),
            FunctionPlaceBase::Local(_)
            | FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::GlobalInstance { .. }
            | FunctionPlaceBase::Error => false,
        }
    }

    pub(super) fn build_place_load<T: nia_llvm::types::AsTypeRef>(
        &self,
        ty: T,
        ptr: PointerValue<'ctx>,
        name: &str,
        is_volatile: bool,
    ) -> nia_llvm::LlvmResult<BasicValueEnum<'ctx>> {
        if is_volatile {
            self.builder.build_volatile_load(ty, ptr, name)
        } else {
            self.builder.build_load(ty, ptr, name)
        }
    }

    pub(super) fn build_place_store<V: nia_llvm::values::BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
        is_volatile: bool,
    ) -> nia_llvm::LlvmResult<nia_llvm::values::InstructionValue<'ctx>> {
        if is_volatile {
            self.builder.build_volatile_store(ptr, value)
        } else {
            self.builder.build_store(ptr, value)
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
        index: &FunctionExpr,
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
            Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
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
                    .into_struct_value()
                    .map_err(|_| self.error(span, "loaded slice base is not a struct"))?;
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
