// SPDX-License-Identifier: GPL-3.0-or-later
//! LLVM instruction builder wrapper.
//!
//! The builder exposes typed constructors for LLVM instructions and constants.
//! Unsafe GEP helpers require callers to pass element types and indices that
//! match the pointee/object being addressed.

use llvm_sys::core::{
    LLVMAddCase, LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAlloca, LLVMBuildAnd,
    LLVMBuildAtomicCmpXchg, LLVMBuildAtomicRMW, LLVMBuildBitCast, LLVMBuildBr, LLVMBuildCall2,
    LLVMBuildCondBr, LLVMBuildExtractElement, LLVMBuildExtractValue, LLVMBuildFAdd, LLVMBuildFCmp,
    LLVMBuildFDiv, LLVMBuildFMul, LLVMBuildFNeg, LLVMBuildFPCast, LLVMBuildFPToSI, LLVMBuildFPToUI,
    LLVMBuildFRem, LLVMBuildFSub, LLVMBuildFence, LLVMBuildGEP2, LLVMBuildICmp,
    LLVMBuildInsertElement, LLVMBuildInsertValue, LLVMBuildIntToPtr, LLVMBuildLShr, LLVMBuildLoad2,
    LLVMBuildMemCpy, LLVMBuildMemMove, LLVMBuildMemSet, LLVMBuildMul, LLVMBuildNeg, LLVMBuildNot,
    LLVMBuildOr, LLVMBuildPhi, LLVMBuildPointerCast, LLVMBuildPtrDiff2, LLVMBuildPtrToInt,
    LLVMBuildRet, LLVMBuildRetVoid, LLVMBuildSDiv, LLVMBuildSExt, LLVMBuildSIToFP, LLVMBuildSRem,
    LLVMBuildSelect, LLVMBuildShl, LLVMBuildShuffleVector, LLVMBuildStore, LLVMBuildStructGEP2,
    LLVMBuildSub, LLVMBuildSwitch, LLVMBuildTrunc, LLVMBuildUDiv, LLVMBuildUIToFP, LLVMBuildURem,
    LLVMBuildUnreachable, LLVMBuildXor, LLVMBuildZExt, LLVMClearInsertionPosition,
    LLVMDisposeBuilder, LLVMGetInsertBlock, LLVMPositionBuilderAtEnd, LLVMPositionBuilderBefore,
    LLVMSetCurrentDebugLocation2,
};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMTypeRef, LLVMValueRef};
use std::marker::PhantomData;

use super::{
    AggregateValue, AsTypeRef, AsValueRef, AtomicOrdering, AtomicRMWBinOp, BasicBlock,
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue, Context, DILocation,
    FloatPredicate, FloatType, FloatValue, FunctionType, FunctionValue, InstructionValue,
    IntPredicate, IntType, IntValue, LlvmResult, PhiValue, PointerType, PointerValue, StructValue,
    VectorValue, to_c_string,
};
pub struct Builder<'ctx> {
    pub(super) raw: LLVMBuilderRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Builder<'ctx> {
    pub(super) fn new(raw: LLVMBuilderRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
    pub fn get_insert_block(&self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetInsertBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    pub fn position_at_end(&self, block: BasicBlock<'ctx>) {
        unsafe { LLVMPositionBuilderAtEnd(self.raw, block.raw) };
    }

    pub fn position_before(&self, instruction: &InstructionValue<'ctx>) {
        unsafe { LLVMPositionBuilderBefore(self.raw, instruction.raw) };
    }

    pub fn clear_insertion_position(&self) {
        unsafe { LLVMClearInsertionPosition(self.raw) };
    }

    pub fn set_current_debug_location(&self, location: DILocation<'ctx>) {
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, location.raw) };
    }

    pub fn clear_current_debug_location(&self) {
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, std::ptr::null_mut()) };
    }

    pub fn build_alloca<T: AsTypeRef>(&self, ty: T, name: &str) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(PointerValue::new(unsafe {
            LLVMBuildAlloca(self.raw, ty.as_type_ref(), name.as_ptr())
        }))
    }

    pub fn build_store<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildStore(self.raw, value.as_value_ref(), ptr.as_value_ref())
        }))
    }

    pub fn build_load<T: AsTypeRef>(
        &self,
        ty: T,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildLoad2(
                self.raw,
                ty.as_type_ref(),
                ptr.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_aligned_load<T: AsTypeRef>(
        &self,
        ty: T,
        ptr: PointerValue<'ctx>,
        align: u32,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let value = self.build_load(ty, ptr, name)?;
        if let Some(inst) = value.as_instruction_value() {
            inst.set_alignment(align);
        }
        Ok(value)
    }

    pub fn build_volatile_store<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let inst = self.build_store(ptr, value)?;
        inst.set_volatile(true);
        Ok(inst)
    }

    pub fn build_volatile_load<T: AsTypeRef>(
        &self,
        ty: T,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let value = self.build_load(ty, ptr, name)?;
        if let Some(inst) = value.as_instruction_value() {
            inst.set_volatile(true);
        }
        Ok(value)
    }

    /// # Safety
    /// `pointee_ty` must match the actual pointee type of `ptr`, and `indexes`
    /// must describe a valid in-bounds GEP for that allocation.
    pub unsafe fn build_gep<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        ptr: PointerValue<'ctx>,
        indexes: &[IntValue<'ctx>],
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        let mut indexes = indexes
            .iter()
            .map(|idx| idx.as_value_ref())
            .collect::<Vec<_>>();
        // SAFETY: The caller upholds the pointee-type and index validity
        // contract for this typed GEP wrapper. The temporary index buffer lives
        // for the duration of the LLVM call.
        Ok(PointerValue::new(unsafe {
            LLVMBuildGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                indexes.as_mut_ptr(),
                indexes.len() as u32,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_struct_gep<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        ptr: PointerValue<'ctx>,
        index: u32,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(PointerValue::new(unsafe {
            LLVMBuildStructGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                index,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_ptr_diff<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildPtrDiff2(
                self.raw,
                pointee_ty.as_type_ref(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_phi<T: AsTypeRef>(&self, ty: T, name: &str) -> LlvmResult<PhiValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(PhiValue::new(unsafe {
            LLVMBuildPhi(self.raw, ty.as_type_ref(), name.as_ptr())
        }))
    }

    pub fn build_call(
        &self,
        function: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> LlvmResult<CallSiteValue<'ctx>> {
        self.build_call2(function.get_type(), function.as_value_ref(), args, name)
    }

    pub fn build_indirect_call(
        &self,
        function_type: FunctionType<'ctx>,
        function_pointer: PointerValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> LlvmResult<CallSiteValue<'ctx>> {
        self.build_call2(function_type, function_pointer.as_value_ref(), args, name)
    }

    fn build_call2(
        &self,
        function_type: FunctionType<'ctx>,
        callee: LLVMValueRef,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> LlvmResult<CallSiteValue<'ctx>> {
        let name = if function_type.get_return_type()?.is_none() {
            ""
        } else {
            name
        };
        let name = to_c_string(name)?;
        let mut args = args
            .iter()
            .map(|arg| arg.as_value_ref())
            .collect::<Vec<_>>();
        Ok(CallSiteValue::new(unsafe {
            LLVMBuildCall2(
                self.raw,
                function_type.as_type_ref(),
                callee,
                args.as_mut_ptr(),
                args.len() as u32,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_return(
        &self,
        value: Option<&dyn BasicValue<'ctx>>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        Ok(InstructionValue::new(unsafe {
            match value {
                Some(value) => LLVMBuildRet(self.raw, value.as_value_ref()),
                None => LLVMBuildRetVoid(self.raw),
            }
        }))
    }

    pub fn build_unreachable(&self) -> LlvmResult<InstructionValue<'ctx>> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildUnreachable(self.raw)
        }))
    }

    pub fn build_unconditional_branch(
        &self,
        destination: BasicBlock<'ctx>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildBr(self.raw, destination.raw)
        }))
    }

    pub fn build_conditional_branch(
        &self,
        comparison: IntValue<'ctx>,
        then_block: BasicBlock<'ctx>,
        else_block: BasicBlock<'ctx>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildCondBr(
                self.raw,
                comparison.as_value_ref(),
                then_block.raw,
                else_block.raw,
            )
        }))
    }

    pub fn build_switch(
        &self,
        value: IntValue<'ctx>,
        else_block: BasicBlock<'ctx>,
        cases: &[(IntValue<'ctx>, BasicBlock<'ctx>)],
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let inst = unsafe {
            LLVMBuildSwitch(
                self.raw,
                value.as_value_ref(),
                else_block.raw,
                cases.len() as u32,
            )
        };
        for (case_value, block) in cases {
            unsafe { LLVMAddCase(inst, case_value.as_value_ref(), block.raw) };
        }
        Ok(InstructionValue::new(inst))
    }

    pub fn build_extract_value<AV: AggregateValue<'ctx>>(
        &self,
        aggregate: AV,
        index: u32,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildExtractValue(self.raw, aggregate.as_value_ref(), index, name.as_ptr())
        })
    }

    pub fn build_insert_value<AV: AggregateValue<'ctx>, BV: BasicValue<'ctx>>(
        &self,
        aggregate: AV,
        value: BV,
        index: u32,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildInsertValue(
                self.raw,
                aggregate.as_value_ref(),
                value.as_value_ref(),
                index,
                name.as_ptr(),
            )
        })
    }

    pub fn build_memcpy(
        &self,
        dest: PointerValue<'ctx>,
        dest_align: u32,
        src: PointerValue<'ctx>,
        src_align: u32,
        size: IntValue<'ctx>,
    ) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(unsafe {
            LLVMBuildMemCpy(
                self.raw,
                dest.as_value_ref(),
                dest_align,
                src.as_value_ref(),
                src_align,
                size.as_value_ref(),
            )
        }))
    }

    pub fn build_memset(
        &self,
        dest: PointerValue<'ctx>,
        align: u32,
        value: IntValue<'ctx>,
        size: IntValue<'ctx>,
    ) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(unsafe {
            LLVMBuildMemSet(
                self.raw,
                dest.as_value_ref(),
                value.as_value_ref(),
                size.as_value_ref(),
                align,
            )
        }))
    }

    pub fn build_memmove(
        &self,
        dest: PointerValue<'ctx>,
        dest_align: u32,
        src: PointerValue<'ctx>,
        src_align: u32,
        size: IntValue<'ctx>,
    ) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(unsafe {
            LLVMBuildMemMove(
                self.raw,
                dest.as_value_ref(),
                dest_align,
                src.as_value_ref(),
                src_align,
                size.as_value_ref(),
            )
        }))
    }

    pub fn build_fence(
        &self,
        ordering: AtomicOrdering,
        sync_scope: i32,
        name: &str,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(InstructionValue::new(unsafe {
            LLVMBuildFence(self.raw, ordering.into(), sync_scope, name.as_ptr())
        }))
    }

    pub fn build_atomicrmw<V: BasicValue<'ctx>>(
        &self,
        op: AtomicRMWBinOp,
        ptr: PointerValue<'ctx>,
        value: V,
        ordering: AtomicOrdering,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe {
            LLVMBuildAtomicRMW(
                self.raw,
                op.into(),
                ptr.as_value_ref(),
                value.as_value_ref(),
                ordering.into(),
                0,
            )
        })
    }

    pub fn build_cmpxchg<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        expected: V,
        desired: V,
        success: AtomicOrdering,
        failure: AtomicOrdering,
        weak: bool,
    ) -> LlvmResult<StructValue<'ctx>> {
        let value = StructValue::new(unsafe {
            LLVMBuildAtomicCmpXchg(
                self.raw,
                ptr.as_value_ref(),
                expected.as_value_ref(),
                desired.as_value_ref(),
                success.into(),
                failure.into(),
                0,
            )
        });
        if weak && let Some(inst) = value.as_instruction() {
            inst.set_weak(true);
        }
        Ok(value)
    }

    pub fn build_int_add(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildAdd, lhs, rhs, name)
    }

    pub fn build_int_sub(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSub, lhs, rhs, name)
    }

    pub fn build_int_mul(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildMul, lhs, rhs, name)
    }

    pub fn build_int_signed_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSDiv, lhs, rhs, name)
    }

    pub fn build_int_unsigned_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildUDiv, lhs, rhs, name)
    }

    pub fn build_int_signed_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSRem, lhs, rhs, name)
    }

    pub fn build_int_unsigned_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildURem, lhs, rhs, name)
    }

    pub fn build_and(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildAnd, lhs, rhs, name)
    }

    pub fn build_or(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildOr, lhs, rhs, name)
    }

    pub fn build_xor(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildXor, lhs, rhs, name)
    }

    pub fn build_left_shift(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildShl, lhs, rhs, name)
    }

    pub fn build_right_shift(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        signed: bool,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        if signed {
            build_int_bin(self.raw, LLVMBuildAShr, lhs, rhs, name)
        } else {
            build_int_bin(self.raw, LLVMBuildLShr, lhs, rhs, name)
        }
    }

    pub fn build_int_compare(
        &self,
        pred: IntPredicate,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildICmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_basic_int_add(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAdd, lhs, rhs, name)
    }

    pub fn build_basic_int_sub(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSub, lhs, rhs, name)
    }

    pub fn build_basic_int_mul(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildMul, lhs, rhs, name)
    }

    pub fn build_basic_int_signed_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSDiv, lhs, rhs, name)
    }

    pub fn build_basic_int_unsigned_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildUDiv, lhs, rhs, name)
    }

    pub fn build_basic_int_signed_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSRem, lhs, rhs, name)
    }

    pub fn build_basic_int_unsigned_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildURem, lhs, rhs, name)
    }

    pub fn build_basic_and(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAnd, lhs, rhs, name)
    }

    pub fn build_basic_or(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildOr, lhs, rhs, name)
    }

    pub fn build_basic_xor(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildXor, lhs, rhs, name)
    }

    pub fn build_basic_shl(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildShl, lhs, rhs, name)
    }

    pub fn build_basic_lshr(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildLShr, lhs, rhs, name)
    }

    pub fn build_basic_ashr(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAShr, lhs, rhs, name)
    }

    pub fn build_basic_int_compare(
        &self,
        pred: IntPredicate,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildICmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_float_add(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFAdd, lhs, rhs, name)
    }

    pub fn build_float_sub(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFSub, lhs, rhs, name)
    }

    pub fn build_float_mul(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFMul, lhs, rhs, name)
    }

    pub fn build_float_div(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFDiv, lhs, rhs, name)
    }

    pub fn build_float_rem(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFRem, lhs, rhs, name)
    }

    pub fn build_float_compare(
        &self,
        pred: FloatPredicate,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildFCmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_basic_float_add(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFAdd, lhs, rhs, name)
    }

    pub fn build_basic_float_sub(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFSub, lhs, rhs, name)
    }

    pub fn build_basic_float_mul(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFMul, lhs, rhs, name)
    }

    pub fn build_basic_float_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFDiv, lhs, rhs, name)
    }

    pub fn build_basic_float_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFRem, lhs, rhs, name)
    }

    pub fn build_basic_float_compare(
        &self,
        pred: FloatPredicate,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildFCmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_int_neg(&self, value: IntValue<'ctx>, name: &str) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildNeg(self.raw, value.as_value_ref(), name.as_ptr())
        }))
    }

    pub fn build_float_neg(
        &self,
        value: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(FloatValue::new(unsafe {
            LLVMBuildFNeg(self.raw, value.as_value_ref(), name.as_ptr())
        }))
    }

    pub fn build_not(&self, value: IntValue<'ctx>, name: &str) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildNot(self.raw, value.as_value_ref(), name.as_ptr())
        }))
    }

    pub fn build_basic_neg(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildNeg(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    pub fn build_basic_float_neg(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildFNeg(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    pub fn build_basic_not(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildNot(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    pub fn build_select(
        &self,
        cond: BasicValueEnum<'ctx>,
        on_true: BasicValueEnum<'ctx>,
        on_false: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildSelect(
                self.raw,
                cond.as_value_ref(),
                on_true.as_value_ref(),
                on_false.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_extract_element(
        &self,
        vector: VectorValue<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildExtractElement(
                self.raw,
                vector.as_value_ref(),
                index.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_insert_element(
        &self,
        vector: VectorValue<'ctx>,
        element: BasicValueEnum<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildInsertElement(
                self.raw,
                vector.as_value_ref(),
                element.as_value_ref(),
                index.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_shuffle_vector(
        &self,
        lhs: VectorValue<'ctx>,
        rhs: VectorValue<'ctx>,
        mask: VectorValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildShuffleVector(
                self.raw,
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                mask.as_value_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_bit_cast<V: BasicValue<'ctx>, T: AsTypeRef>(
        &self,
        value: V,
        target: T,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildBitCast(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        })
    }

    pub fn build_pointer_cast(
        &self,
        value: PointerValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(PointerValue::new(unsafe {
            LLVMBuildPointerCast(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_ptr_to_int(
        &self,
        value: PointerValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(IntValue::new(unsafe {
            LLVMBuildPtrToInt(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_int_to_ptr(
        &self,
        value: IntValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        Ok(PointerValue::new(unsafe {
            LLVMBuildIntToPtr(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_int_z_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(self.raw, LLVMBuildZExt, value, target, name)
    }

    pub fn build_int_s_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(self.raw, LLVMBuildSExt, value, target, name)
    }

    pub fn build_int_truncate(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(self.raw, LLVMBuildTrunc, value, target, name)
    }

    pub fn build_signed_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        cast_float(self.raw, LLVMBuildSIToFP, value, target, name)
    }

    pub fn build_unsigned_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        cast_float(self.raw, LLVMBuildUIToFP, value, target, name)
    }

    pub fn build_float_to_signed_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int_from_float(self.raw, LLVMBuildFPToSI, value, target, name)
    }

    pub fn build_float_to_unsigned_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int_from_float(self.raw, LLVMBuildFPToUI, value, target, name)
    }

    pub fn build_float_cast(
        &self,
        value: FloatValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        cast_float(self.raw, LLVMBuildFPCast, value, target, name)
    }
}

impl<'ctx> Drop for Builder<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeBuilder(self.raw) };
    }
}

fn build_int_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: IntValue<'ctx>,
    rhs: IntValue<'ctx>,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    let name = to_c_string(name)?;
    Ok(IntValue::new(unsafe {
        f(
            builder,
            lhs.as_value_ref(),
            rhs.as_value_ref(),
            name.as_ptr(),
        )
    }))
}

fn build_float_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: FloatValue<'ctx>,
    rhs: FloatValue<'ctx>,
    name: &str,
) -> LlvmResult<FloatValue<'ctx>> {
    let name = to_c_string(name)?;
    Ok(FloatValue::new(unsafe {
        f(
            builder,
            lhs.as_value_ref(),
            rhs.as_value_ref(),
            name.as_ptr(),
        )
    }))
}

fn build_basic_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: BasicValueEnum<'ctx>,
    rhs: BasicValueEnum<'ctx>,
    name: &str,
) -> LlvmResult<BasicValueEnum<'ctx>> {
    let name = to_c_string(name)?;
    BasicValueEnum::new(unsafe {
        f(
            builder,
            lhs.as_value_ref(),
            rhs.as_value_ref(),
            name.as_ptr(),
        )
    })
}

fn cast_int<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: IntValue<'ctx>,
    target: IntType<'ctx>,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    let name = to_c_string(name)?;
    Ok(IntValue::new(unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    }))
}

fn cast_float<'ctx, V: AsValueRef>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: V,
    target: FloatType<'ctx>,
    name: &str,
) -> LlvmResult<FloatValue<'ctx>> {
    let name = to_c_string(name)?;
    Ok(FloatValue::new(unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    }))
}

fn cast_int_from_float<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: FloatValue<'ctx>,
    target: IntType<'ctx>,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    let name = to_c_string(name)?;
    Ok(IntValue::new(unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    }))
}
