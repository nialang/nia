// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionExpr, FunctionMemoryIntrinsic, FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource,
};
use nia_llvm::{
    IntPredicate,
    values::{IntValue, PointerValue, StructValue},
};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_memory_intrinsic(
        &mut self,
        memory: &FunctionMemoryIntrinsic,
    ) -> Result<(), Diagnostic> {
        let Some(layout) = self.module.layout_of(memory.elem_ty) else {
            return Err(self.error(memory.span, "memory intrinsic element type has no layout"));
        };

        let (dest_ptr, dest_len) = self.emit_slice_operand(memory.span, &memory.dest)?;
        match (&memory.op, &memory.source) {
            (
                FunctionMemoryIntrinsicOp::Copy | FunctionMemoryIntrinsicOp::Move,
                FunctionMemoryIntrinsicSource::Slice(source),
            ) => {
                let (source_ptr, source_len) = self.emit_slice_operand(memory.span, source)?;
                if layout.size == 0 {
                    return Ok(());
                }
                let len = self.min_slice_len(memory.span, dest_len, source_len)?;
                let size = self.byte_len(memory.span, len, layout.size)?;
                // Slice elements keep their type's alignment at both ends.
                let align = self.element_align(memory.span, layout.align)?;
                match memory.op {
                    FunctionMemoryIntrinsicOp::Copy => self
                        .builder
                        .build_memcpy(dest_ptr, align, source_ptr, align, size)
                        .map_err(|_| self.error(memory.span, "failed to build memcpy"))?,
                    FunctionMemoryIntrinsicOp::Move => self
                        .builder
                        .build_memmove(dest_ptr, align, source_ptr, align, size)
                        .map_err(|_| self.error(memory.span, "failed to build memmove"))?,
                    FunctionMemoryIntrinsicOp::Set => {
                        return Err(
                            self.error(memory.span, "memory set operation lacks a byte source")
                        );
                    }
                };
            }
            (FunctionMemoryIntrinsicOp::Set, FunctionMemoryIntrinsicSource::Byte(value)) => {
                self.require_memset_byte_elem(memory.elem_ty, memory.span)?;
                let value = self.emit_expr(value)?.into_int_value()?;
                let size = self.byte_len(memory.span, dest_len, layout.size)?;
                self.builder
                    .build_memset(dest_ptr, 1, value, size)
                    .map_err(|_| self.error(memory.span, "failed to build memset"))?;
            }
            _ => {
                return Err(self.error(
                    memory.span,
                    "memory intrinsic source does not match operation",
                ));
            }
        }
        Ok(())
    }

    fn emit_slice_operand(
        &mut self,
        span: Span,
        expr: &FunctionExpr,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>), Diagnostic> {
        let value = self.emit_expr(expr)?;
        let slice = value
            .into_struct_value()
            .map_err(|_| self.error(span, "memory intrinsic operand is not a slice"))?;
        self.extract_slice_operand(span, slice)
    }

    fn extract_slice_operand(
        &self,
        span: Span,
        slice: StructValue<'ctx>,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>), Diagnostic> {
        let ptr = self.extract_slice_ptr(span, slice)?;
        let len = self.extract_slice_len(span, slice)?;
        Ok((ptr, len))
    }

    fn min_slice_len(
        &self,
        span: Span,
        dest_len: IntValue<'ctx>,
        source_len: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        let dest_shorter = self
            .builder
            .build_int_compare(IntPredicate::ULT, dest_len, source_len, "mem.dest.shorter")
            .map_err(|_| self.error(span, "failed to compare memory intrinsic lengths"))?;
        self.builder
            .build_select(
                dest_shorter.into(),
                dest_len.into(),
                source_len.into(),
                "mem.copy.len",
            )
            .map_err(|_| self.error(span, "failed to select memory intrinsic length"))?
            .into_int_value()
            .map_err(|_| self.error(span, "memory intrinsic length is not an integer"))
    }

    fn byte_len(
        &self,
        span: Span,
        len: IntValue<'ctx>,
        elem_size: u64,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        if elem_size == 1 {
            return Ok(len);
        }
        let size = self
            .module
            .usize_llvm_type(span)?
            .const_int(elem_size, false)?;
        self.builder
            .build_int_mul(len, size, "mem.bytes")
            .map_err(|_| self.error(span, "failed to compute memory intrinsic byte length"))
    }

    fn element_align(&self, span: Span, align: u64) -> Result<u32, Diagnostic> {
        u32::try_from(align)
            .map_err(|_| self.error(span, "memory intrinsic alignment is too large"))
    }

    fn require_memset_byte_elem(
        &self,
        elem_ty: nia_ids::InternedTyId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match self.module.ty_kind(elem_ty) {
            Some(TyKind::Primitive(PrimitiveTy::U8)) => Ok(()),
            _ => Err(self.error(span, "std::builtin::memset destination element must be u8")),
        }
    }
}
