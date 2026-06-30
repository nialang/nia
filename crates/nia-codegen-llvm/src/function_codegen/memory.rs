// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionExpr, FunctionMemoryIntrinsic, FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource,
};
use nia_llvm::{
    IntPredicate,
    basic_block::BasicBlock,
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
                match memory.op {
                    FunctionMemoryIntrinsicOp::Copy => {
                        self.emit_forward_byte_copy(memory.span, dest_ptr, source_ptr, size)?;
                    }
                    FunctionMemoryIntrinsicOp::Move => {
                        self.emit_overlapping_byte_copy(memory.span, dest_ptr, source_ptr, size)?;
                    }
                    FunctionMemoryIntrinsicOp::Set => unreachable!(),
                }
            }
            (FunctionMemoryIntrinsicOp::Set, FunctionMemoryIntrinsicSource::Byte(value)) => {
                self.require_memset_byte_elem(memory.elem_ty, memory.span)?;
                let value = self.emit_expr(value)?.into_int_value()?;
                let size = self.byte_len(memory.span, dest_len, layout.size)?;
                self.emit_byte_set(memory.span, dest_ptr, value, size)?;
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

    fn emit_forward_byte_copy(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        source: PointerValue<'ctx>,
        size: IntValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let after = self
            .module
            .context
            .append_basic_block(self.llvm_function, "memcpy.end")
            .map_err(|_| self.error(span, "failed to create memcpy end block"))?;
        self.emit_forward_byte_copy_to(span, dest, source, size, after)?;
        self.builder.position_at_end(after);
        Ok(())
    }

    fn emit_overlapping_byte_copy(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        source: PointerValue<'ctx>,
        size: IntValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let forward = self
            .module
            .context
            .append_basic_block(self.llvm_function, "memmove.forward")
            .map_err(|_| self.error(span, "failed to create memmove forward block"))?;
        let backward = self
            .module
            .context
            .append_basic_block(self.llvm_function, "memmove.backward")
            .map_err(|_| self.error(span, "failed to create memmove backward block"))?;
        let after = self
            .module
            .context
            .append_basic_block(self.llvm_function, "memmove.end")
            .map_err(|_| self.error(span, "failed to create memmove end block"))?;
        let usize_ty = self.module.context.i64_type();
        let dest_addr = self
            .builder
            .build_ptr_to_int(dest, usize_ty, "memmove.dest")
            .map_err(|_| self.error(span, "failed to cast memmove destination"))?;
        let source_addr = self
            .builder
            .build_ptr_to_int(source, usize_ty, "memmove.source")
            .map_err(|_| self.error(span, "failed to cast memmove source"))?;
        let use_forward = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                dest_addr,
                source_addr,
                "memmove.forward?",
            )
            .map_err(|_| self.error(span, "failed to compare memmove pointers"))?;
        self.builder
            .build_conditional_branch(use_forward, forward, backward)
            .map_err(|_| self.error(span, "failed to branch for memmove direction"))?;

        self.builder.position_at_end(forward);
        self.emit_forward_byte_copy_to(span, dest, source, size, after)?;

        self.builder.position_at_end(backward);
        self.emit_backward_byte_copy_to(span, dest, source, size, after)?;

        self.builder.position_at_end(after);
        Ok(())
    }

    fn emit_byte_set(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        value: IntValue<'ctx>,
        size: IntValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let after = self
            .module
            .context
            .append_basic_block(self.llvm_function, "memset.end")
            .map_err(|_| self.error(span, "failed to create memset end block"))?;
        self.emit_byte_set_to(span, dest, value, size, after)?;
        self.builder.position_at_end(after);
        Ok(())
    }

    fn emit_forward_byte_copy_to(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        source: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        after: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        self.emit_byte_loop_to(span, size, after, |this, index| {
            let byte_ty = this.module.context.i8_type();
            let dest_ptr = unsafe {
                this.builder
                    .build_gep(byte_ty, dest, &[index], "memcpy.dest")
                    .map_err(|_| this.error(span, "failed to compute memcpy destination"))?
            };
            let source_ptr = unsafe {
                this.builder
                    .build_gep(byte_ty, source, &[index], "memcpy.source")
                    .map_err(|_| this.error(span, "failed to compute memcpy source"))?
            };
            let byte = this
                .builder
                .build_load(byte_ty, source_ptr, "memcpy.byte")
                .map_err(|_| this.error(span, "failed to load memcpy byte"))?;
            this.builder
                .build_store(dest_ptr, byte)
                .map_err(|_| this.error(span, "failed to store memcpy byte"))?;
            Ok(())
        })
    }

    fn emit_backward_byte_copy_to(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        source: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        after: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        self.emit_reverse_byte_loop_to(span, size, after, |this, index| {
            let byte_ty = this.module.context.i8_type();
            let dest_ptr = unsafe {
                this.builder
                    .build_gep(byte_ty, dest, &[index], "memmove.dest")
                    .map_err(|_| this.error(span, "failed to compute memmove destination"))?
            };
            let source_ptr = unsafe {
                this.builder
                    .build_gep(byte_ty, source, &[index], "memmove.source")
                    .map_err(|_| this.error(span, "failed to compute memmove source"))?
            };
            let byte = this
                .builder
                .build_load(byte_ty, source_ptr, "memmove.byte")
                .map_err(|_| this.error(span, "failed to load memmove byte"))?;
            this.builder
                .build_store(dest_ptr, byte)
                .map_err(|_| this.error(span, "failed to store memmove byte"))?;
            Ok(())
        })
    }

    fn emit_byte_set_to(
        &mut self,
        span: Span,
        dest: PointerValue<'ctx>,
        value: IntValue<'ctx>,
        size: IntValue<'ctx>,
        after: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        self.emit_byte_loop_to(span, size, after, |this, index| {
            let byte_ty = this.module.context.i8_type();
            let dest_ptr = unsafe {
                this.builder
                    .build_gep(byte_ty, dest, &[index], "memset.dest")
                    .map_err(|_| this.error(span, "failed to compute memset destination"))?
            };
            this.builder
                .build_store(dest_ptr, value)
                .map_err(|_| this.error(span, "failed to store memset byte"))?;
            Ok(())
        })
    }

    fn emit_byte_loop_to(
        &mut self,
        span: Span,
        size: IntValue<'ctx>,
        after: BasicBlock<'ctx>,
        mut emit_body: impl FnMut(&mut Self, IntValue<'ctx>) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let header = self
            .module
            .context
            .append_basic_block(self.llvm_function, "mem.loop")
            .map_err(|_| self.error(span, "failed to create memory loop header"))?;
        let body = self
            .module
            .context
            .append_basic_block(self.llvm_function, "mem.body")
            .map_err(|_| self.error(span, "failed to create memory loop body"))?;
        let index_ptr = self.alloc_loop_index(span, "mem.i")?;
        let zero = self.module.context.i64_type().const_int(0, false);
        let one = self.module.context.i64_type().const_int(1, false);
        self.builder
            .build_store(index_ptr, zero)
            .map_err(|_| self.error(span, "failed to initialize memory loop index"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|_| self.error(span, "failed to enter memory loop"))?;

        self.builder.position_at_end(header);
        let index = self.load_loop_index(span, index_ptr)?;
        let keep_going = self
            .builder
            .build_int_compare(IntPredicate::ULT, index, size, "mem.more")
            .map_err(|_| self.error(span, "failed to compare memory loop index"))?;
        self.builder
            .build_conditional_branch(keep_going, body, after)
            .map_err(|_| self.error(span, "failed to branch memory loop"))?;

        self.builder.position_at_end(body);
        emit_body(self, index)?;
        let next = self
            .builder
            .build_int_add(index, one, "mem.next")
            .map_err(|_| self.error(span, "failed to increment memory loop index"))?;
        self.builder
            .build_store(index_ptr, next)
            .map_err(|_| self.error(span, "failed to store memory loop index"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|_| self.error(span, "failed to continue memory loop"))?;
        Ok(())
    }

    fn emit_reverse_byte_loop_to(
        &mut self,
        span: Span,
        size: IntValue<'ctx>,
        after: BasicBlock<'ctx>,
        mut emit_body: impl FnMut(&mut Self, IntValue<'ctx>) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let header = self
            .module
            .context
            .append_basic_block(self.llvm_function, "mem.rev.loop")
            .map_err(|_| self.error(span, "failed to create reverse memory loop header"))?;
        let body = self
            .module
            .context
            .append_basic_block(self.llvm_function, "mem.rev.body")
            .map_err(|_| self.error(span, "failed to create reverse memory loop body"))?;
        let index_ptr = self.alloc_loop_index(span, "mem.rev.i")?;
        let zero = self.module.context.i64_type().const_int(0, false);
        let one = self.module.context.i64_type().const_int(1, false);
        self.builder
            .build_store(index_ptr, size)
            .map_err(|_| self.error(span, "failed to initialize reverse memory loop index"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|_| self.error(span, "failed to enter reverse memory loop"))?;

        self.builder.position_at_end(header);
        let index = self.load_loop_index(span, index_ptr)?;
        let keep_going = self
            .builder
            .build_int_compare(IntPredicate::NE, index, zero, "mem.rev.more")
            .map_err(|_| self.error(span, "failed to compare reverse memory loop index"))?;
        self.builder
            .build_conditional_branch(keep_going, body, after)
            .map_err(|_| self.error(span, "failed to branch reverse memory loop"))?;

        self.builder.position_at_end(body);
        let next = self
            .builder
            .build_int_sub(index, one, "mem.rev.next")
            .map_err(|_| self.error(span, "failed to decrement reverse memory loop index"))?;
        emit_body(self, next)?;
        self.builder
            .build_store(index_ptr, next)
            .map_err(|_| self.error(span, "failed to store reverse memory loop index"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|_| self.error(span, "failed to continue reverse memory loop"))?;
        Ok(())
    }

    fn alloc_loop_index(&self, span: Span, name: &str) -> Result<PointerValue<'ctx>, Diagnostic> {
        self.builder
            .build_alloca(self.module.context.i64_type(), name)
            .map_err(|_| self.error(span, "failed to allocate memory loop index"))
    }

    fn load_loop_index(
        &self,
        span: Span,
        ptr: PointerValue<'ctx>,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        self.builder
            .build_load(self.module.context.i64_type(), ptr, "mem.i.load")
            .map_err(|_| self.error(span, "failed to load memory loop index"))?
            .into_int_value()
            .map_err(Into::into)
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
        let size = self.module.context.i64_type().const_int(elem_size, false);
        self.builder
            .build_int_mul(len, size, "mem.bytes")
            .map_err(|_| self.error(span, "failed to compute memory intrinsic byte length"))
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
