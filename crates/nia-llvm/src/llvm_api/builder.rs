// SPDX-License-Identifier: GPL-3.0-or-later
//! LLVM instruction builder wrapper.
//!
//! The builder exposes typed constructors for LLVM instructions and constants.
//! Unsafe GEP helpers require callers to pass element types and indices that
//! match the pointee/object being addressed.

use llvm_sys::LLVMTypeKind;
use llvm_sys::core::{
    LLVMAddCase, LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAlloca, LLVMBuildAnd,
    LLVMBuildAtomicCmpXchg, LLVMBuildAtomicRMW, LLVMBuildBitCast, LLVMBuildBr, LLVMBuildCall2,
    LLVMBuildCondBr, LLVMBuildExtractElement, LLVMBuildExtractValue, LLVMBuildFAdd, LLVMBuildFCmp,
    LLVMBuildFDiv, LLVMBuildFMul, LLVMBuildFNeg, LLVMBuildFPCast, LLVMBuildFPToSI, LLVMBuildFPToUI,
    LLVMBuildFRem, LLVMBuildFSub, LLVMBuildFence, LLVMBuildGEP2, LLVMBuildICmp,
    LLVMBuildInsertElement, LLVMBuildInsertValue, LLVMBuildIntToPtr, LLVMBuildLShr, LLVMBuildLoad2,
    LLVMBuildMul, LLVMBuildNeg, LLVMBuildNot, LLVMBuildOr, LLVMBuildPhi, LLVMBuildPointerCast,
    LLVMBuildPtrToInt, LLVMBuildRet, LLVMBuildRetVoid, LLVMBuildSDiv, LLVMBuildSExt,
    LLVMBuildSIToFP, LLVMBuildSRem, LLVMBuildSelect, LLVMBuildShl, LLVMBuildShuffleVector,
    LLVMBuildStore, LLVMBuildStructGEP2, LLVMBuildSub, LLVMBuildSwitch, LLVMBuildTrunc,
    LLVMBuildUDiv, LLVMBuildUIToFP, LLVMBuildURem, LLVMBuildUnreachable, LLVMBuildXor,
    LLVMBuildZExt, LLVMClearInsertionPosition, LLVMCountStructElementTypes, LLVMDisposeBuilder,
    LLVMGetArrayLength2, LLVMGetElementType, LLVMGetInsertBlock, LLVMGetIntTypeWidth,
    LLVMGetTypeKind, LLVMGetVectorSize, LLVMPositionBuilderAtEnd, LLVMPositionBuilderBefore,
    LLVMSetCurrentDebugLocation2, LLVMStructGetTypeAtIndex, LLVMTypeOf,
};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMTypeRef, LLVMValueRef};
use std::marker::PhantomData;

use super::{
    AggregateValue, AsTypeRef, AsValueRef, AtomicOrdering, AtomicRMWBinOp, BasicBlock,
    BasicMetadataValueEnum, BasicTypeEnum, BasicValue, BasicValueEnum, CallSiteValue, Context,
    DILocation, FloatPredicate, FloatType, FloatValue, FunctionType, FunctionValue,
    InstructionValue, IntPredicate, IntType, IntValue, LlvmError, LlvmResult, PhiValue,
    PointerType, PointerValue, StructValue, VectorValue, to_c_string,
};
/// Owned LLVM instruction builder tied to its originating context.
///
/// Every value-producing operation validates LLVM's returned handle before it
/// constructs a typed wrapper. Callers remain responsible for positioning the
/// builder in a live basic block.
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
    /// Returns the block containing the current insertion point.
    pub fn get_insert_block(&self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetInsertBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    /// Moves the insertion point to the end of `block`.
    pub fn position_at_end(&self, block: BasicBlock<'ctx>) {
        unsafe { LLVMPositionBuilderAtEnd(self.raw, block.raw) };
    }

    /// Moves the insertion point immediately before `instruction`.
    pub fn position_before(&self, instruction: &InstructionValue<'ctx>) {
        unsafe { LLVMPositionBuilderBefore(self.raw, instruction.raw) };
    }

    /// Clears the insertion point so no block is selected.
    pub fn clear_insertion_position(&self) {
        unsafe { LLVMClearInsertionPosition(self.raw) };
    }

    /// Applies `location` to subsequently emitted instructions.
    pub fn set_current_debug_location(&self, location: DILocation<'ctx>) {
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, location.raw) };
    }

    /// Stops attaching a debug location to new instructions.
    pub fn clear_current_debug_location(&self) {
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, std::ptr::null_mut()) };
    }

    /// Allocates stack storage for one value of `ty`.
    pub fn build_alloca<T: AsTypeRef>(&self, ty: T, name: &str) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMBuildAlloca(self.raw, ty.as_type_ref(), name.as_ptr()) };
        Ok(PointerValue::new(require_value(value, "alloca")?))
    }

    /// Stores `value` through `ptr`.
    pub fn build_store<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let instruction =
            unsafe { LLVMBuildStore(self.raw, value.as_value_ref(), ptr.as_value_ref()) };
        Ok(InstructionValue::new(require_value(instruction, "store")?))
    }

    /// Loads a value of `ty` through `ptr`.
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

    /// Loads a value while asserting the pointer's byte alignment.
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

    /// Emits a volatile store.
    pub fn build_volatile_store<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let inst = self.build_store(ptr, value)?;
        inst.set_volatile(true);
        Ok(inst)
    }

    /// Emits a volatile load of `ty`.
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
        let value = unsafe {
            LLVMBuildGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                indexes.as_mut_ptr(),
                indexes.len() as u32,
                name.as_ptr(),
            )
        };
        Ok(PointerValue::new(require_value(value, "GEP")?))
    }

    /// Computes the address of a physical struct field.
    ///
    /// # Safety
    /// `pointee_ty` must be the actual struct type addressed by `ptr`, and
    /// `index` must be less than that struct's field count. The pointer must
    /// retain provenance to an allocation whose layout is compatible with the
    /// supplied type. LLVM's opaque-pointer API cannot prove these conditions
    /// from the handles alone, so callers must establish them before calling.
    pub unsafe fn build_struct_gep<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        ptr: PointerValue<'ctx>,
        index: u32,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        // SAFETY: The caller upholds the struct type, field index, and pointer
        // provenance contract documented above.
        let value = unsafe {
            LLVMBuildStructGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                index,
                name.as_ptr(),
            )
        };
        Ok(PointerValue::new(require_value(value, "struct GEP")?))
    }

    /// Creates an empty phi node whose incoming edges are added separately.
    pub fn build_phi<T: AsTypeRef>(&self, ty: T, name: &str) -> LlvmResult<PhiValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMBuildPhi(self.raw, ty.as_type_ref(), name.as_ptr()) };
        Ok(PhiValue::new(require_value(value, "phi")?))
    }

    /// Calls a directly declared function.
    pub fn build_call(
        &self,
        function: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> LlvmResult<CallSiteValue<'ctx>> {
        self.build_call2(function.get_type(), function.as_value_ref(), args, name)
    }

    /// Calls a function pointer using the explicit signature.
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
        let call = unsafe {
            LLVMBuildCall2(
                self.raw,
                function_type.as_type_ref(),
                callee,
                args.as_mut_ptr(),
                args.len() as u32,
                name.as_ptr(),
            )
        };
        Ok(CallSiteValue::new(require_value(call, "call")?))
    }

    /// Returns an optional first-class value from the current function.
    pub fn build_return(
        &self,
        value: Option<&dyn BasicValue<'ctx>>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let instruction = unsafe {
            match value {
                Some(value) => LLVMBuildRet(self.raw, value.as_value_ref()),
                None => LLVMBuildRetVoid(self.raw),
            }
        };
        Ok(InstructionValue::new(require_value(instruction, "return")?))
    }

    /// Terminates the current block as unreachable.
    pub fn build_unreachable(&self) -> LlvmResult<InstructionValue<'ctx>> {
        let instruction = unsafe { LLVMBuildUnreachable(self.raw) };
        Ok(InstructionValue::new(require_value(
            instruction,
            "unreachable",
        )?))
    }

    /// Branches unconditionally to `destination`.
    pub fn build_unconditional_branch(
        &self,
        destination: BasicBlock<'ctx>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let instruction = unsafe { LLVMBuildBr(self.raw, destination.raw) };
        Ok(InstructionValue::new(require_value(instruction, "branch")?))
    }

    /// Branches according to a one-bit integer condition.
    pub fn build_conditional_branch(
        &self,
        comparison: IntValue<'ctx>,
        then_block: BasicBlock<'ctx>,
        else_block: BasicBlock<'ctx>,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let instruction = unsafe {
            LLVMBuildCondBr(
                self.raw,
                comparison.as_value_ref(),
                then_block.raw,
                else_block.raw,
            )
        };
        Ok(InstructionValue::new(require_value(
            instruction,
            "conditional branch",
        )?))
    }

    /// Emits an integer switch and attaches all constant case edges.
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
        let inst = require_value(inst, "switch")?;
        for (case_value, block) in cases {
            unsafe { LLVMAddCase(inst, case_value.as_value_ref(), block.raw) };
        }
        Ok(InstructionValue::new(inst))
    }

    /// Extracts aggregate field `index` after checking its field bounds.
    pub fn build_extract_value<AV: AggregateValue<'ctx>>(
        &self,
        aggregate: AV,
        index: u32,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        aggregate_element_type(aggregate.as_value_ref(), index)?;
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe {
            LLVMBuildExtractValue(self.raw, aggregate.as_value_ref(), index, name.as_ptr())
        })
    }

    /// Returns an aggregate with field `index` replaced by a matching `value`.
    pub fn build_insert_value<AV: AggregateValue<'ctx>, BV: BasicValue<'ctx>>(
        &self,
        aggregate: AV,
        value: BV,
        index: u32,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let field_ty = aggregate_element_type(aggregate.as_value_ref(), index)?;
        let value_ty = require_type(unsafe { LLVMTypeOf(value.as_value_ref()) }, "insert value")?;
        if field_ty != value_ty {
            return Err(LlvmError::error(
                "LLVM aggregate insertion value type does not match the selected field",
            ));
        }
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

    /// Emits an atomic fence with the selected ordering and sync scope.
    pub fn build_fence(
        &self,
        ordering: AtomicOrdering,
        sync_scope: i32,
        name: &str,
    ) -> LlvmResult<InstructionValue<'ctx>> {
        let name = to_c_string(name)?;
        let instruction =
            unsafe { LLVMBuildFence(self.raw, ordering.into(), sync_scope, name.as_ptr()) };
        Ok(InstructionValue::new(require_value(instruction, "fence")?))
    }

    /// Atomically applies `op` and returns the previous memory value.
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

    /// Atomically compares with `expected` and conditionally stores `desired`.
    pub fn build_cmpxchg<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        expected: V,
        desired: V,
        success: AtomicOrdering,
        failure: AtomicOrdering,
        weak: bool,
    ) -> LlvmResult<StructValue<'ctx>> {
        let value = unsafe {
            LLVMBuildAtomicCmpXchg(
                self.raw,
                ptr.as_value_ref(),
                expected.as_value_ref(),
                desired.as_value_ref(),
                success.into(),
                failure.into(),
                0,
            )
        };
        let value = StructValue::new(require_value(value, "atomic compare-exchange")?);
        if weak && let Some(inst) = value.as_instruction() {
            inst.set_weak(true);
        }
        Ok(value)
    }

    /// Adds two scalar integers.
    pub fn build_int_add(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildAdd, lhs, rhs, name)
    }

    /// Subtracts two scalar integers.
    pub fn build_int_sub(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSub, lhs, rhs, name)
    }

    /// Multiplies two scalar integers.
    pub fn build_int_mul(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildMul, lhs, rhs, name)
    }

    /// Divides two scalar integers using signed interpretation.
    pub fn build_int_signed_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSDiv, lhs, rhs, name)
    }

    /// Divides two scalar integers using unsigned interpretation.
    pub fn build_int_unsigned_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildUDiv, lhs, rhs, name)
    }

    /// Computes signed integer remainder.
    pub fn build_int_signed_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildSRem, lhs, rhs, name)
    }

    /// Computes unsigned integer remainder.
    pub fn build_int_unsigned_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildURem, lhs, rhs, name)
    }

    /// Computes scalar integer bitwise AND.
    pub fn build_and(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildAnd, lhs, rhs, name)
    }

    /// Computes scalar integer bitwise OR.
    pub fn build_or(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildOr, lhs, rhs, name)
    }

    /// Computes scalar integer bitwise XOR.
    pub fn build_xor(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildXor, lhs, rhs, name)
    }

    /// Shifts a scalar integer left without masking the count.
    pub fn build_left_shift(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        build_int_bin(self.raw, LLVMBuildShl, lhs, rhs, name)
    }

    /// Shifts right arithmetically when `signed`, logically otherwise.
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

    /// Compares scalar integers using `predicate`'s signedness.
    pub fn build_int_compare(
        &self,
        pred: IntPredicate,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            LLVMBuildICmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        };
        Ok(IntValue::new(require_value(value, "integer comparison")?))
    }

    /// Adds integer scalars or vectors represented by the shared value enum.
    pub fn build_basic_int_add(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAdd, lhs, rhs, name)
    }

    /// Subtracts integer scalars or vectors.
    pub fn build_basic_int_sub(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSub, lhs, rhs, name)
    }

    /// Multiplies integer scalars or vectors.
    pub fn build_basic_int_mul(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildMul, lhs, rhs, name)
    }

    /// Performs signed division on integer scalars or vectors.
    pub fn build_basic_int_signed_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSDiv, lhs, rhs, name)
    }

    /// Performs unsigned division on integer scalars or vectors.
    pub fn build_basic_int_unsigned_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildUDiv, lhs, rhs, name)
    }

    /// Computes signed remainder on integer scalars or vectors.
    pub fn build_basic_int_signed_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildSRem, lhs, rhs, name)
    }

    /// Computes unsigned remainder on integer scalars or vectors.
    pub fn build_basic_int_unsigned_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildURem, lhs, rhs, name)
    }

    /// Computes bitwise AND on integer scalars or vectors.
    pub fn build_basic_and(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAnd, lhs, rhs, name)
    }

    /// Computes bitwise OR on integer scalars or vectors.
    pub fn build_basic_or(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildOr, lhs, rhs, name)
    }

    /// Computes bitwise XOR on integer scalars or vectors.
    pub fn build_basic_xor(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildXor, lhs, rhs, name)
    }

    /// Shifts integer scalars or vectors left.
    pub fn build_basic_shl(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildShl, lhs, rhs, name)
    }

    /// Shifts integer scalars or vectors right logically.
    pub fn build_basic_lshr(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildLShr, lhs, rhs, name)
    }

    /// Shifts integer scalars or vectors right arithmetically.
    pub fn build_basic_ashr(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildAShr, lhs, rhs, name)
    }

    /// Compares integer scalars or vectors with `predicate`.
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

    /// Adds two scalar floating-point values.
    pub fn build_float_add(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFAdd, lhs, rhs, name)
    }

    /// Subtracts two scalar floating-point values.
    pub fn build_float_sub(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFSub, lhs, rhs, name)
    }

    /// Multiplies two scalar floating-point values.
    pub fn build_float_mul(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFMul, lhs, rhs, name)
    }

    /// Divides two scalar floating-point values.
    pub fn build_float_div(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFDiv, lhs, rhs, name)
    }

    /// Computes scalar floating-point remainder.
    pub fn build_float_rem(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        build_float_bin(self.raw, LLVMBuildFRem, lhs, rhs, name)
    }

    /// Performs an ordered scalar floating-point comparison.
    pub fn build_float_compare(
        &self,
        pred: FloatPredicate,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            LLVMBuildFCmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        };
        Ok(IntValue::new(require_value(value, "floating comparison")?))
    }

    /// Adds floating-point scalars or vectors.
    pub fn build_basic_float_add(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFAdd, lhs, rhs, name)
    }

    /// Subtracts floating-point scalars or vectors.
    pub fn build_basic_float_sub(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFSub, lhs, rhs, name)
    }

    /// Multiplies floating-point scalars or vectors.
    pub fn build_basic_float_mul(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFMul, lhs, rhs, name)
    }

    /// Divides floating-point scalars or vectors.
    pub fn build_basic_float_div(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFDiv, lhs, rhs, name)
    }

    /// Computes remainder on floating-point scalars or vectors.
    pub fn build_basic_float_rem(
        &self,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        build_basic_bin(self.raw, LLVMBuildFRem, lhs, rhs, name)
    }

    /// Performs an ordered comparison on floating-point scalars or vectors.
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

    /// Negates a scalar integer.
    pub fn build_int_neg(&self, value: IntValue<'ctx>, name: &str) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMBuildNeg(self.raw, value.as_value_ref(), name.as_ptr()) };
        Ok(IntValue::new(require_value(value, "integer negation")?))
    }

    /// Negates a scalar floating-point value.
    pub fn build_float_neg(
        &self,
        value: FloatValue<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMBuildFNeg(self.raw, value.as_value_ref(), name.as_ptr()) };
        Ok(FloatValue::new(require_value(value, "floating negation")?))
    }

    /// Computes scalar integer bitwise NOT.
    pub fn build_not(&self, value: IntValue<'ctx>, name: &str) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMBuildNot(self.raw, value.as_value_ref(), name.as_ptr()) };
        Ok(IntValue::new(require_value(value, "integer not")?))
    }

    /// Negates integer scalars or vectors.
    pub fn build_basic_neg(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildNeg(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    /// Negates floating-point scalars or vectors.
    pub fn build_basic_float_neg(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildFNeg(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    /// Computes bitwise NOT on integer scalars or vectors.
    pub fn build_basic_not(
        &self,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let name = to_c_string(name)?;
        BasicValueEnum::new(unsafe { LLVMBuildNot(self.raw, value.as_value_ref(), name.as_ptr()) })
    }

    /// Selects `then_value` or `else_value` from a scalar/vector condition.
    pub fn build_select(
        &self,
        cond: BasicValueEnum<'ctx>,
        on_true: BasicValueEnum<'ctx>,
        on_false: BasicValueEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        validate_select_types(
            cond.as_value_ref(),
            on_true.as_value_ref(),
            on_false.as_value_ref(),
        )?;
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

    /// Extracts one vector lane by dynamic integer index.
    pub fn build_extract_element(
        &self,
        vector: VectorValue<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        validate_vector_index(vector.as_value_ref(), index.as_value_ref())?;
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

    /// Returns a vector with one lane replaced.
    pub fn build_insert_element(
        &self,
        vector: VectorValue<'ctx>,
        element: BasicValueEnum<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        let element_ty = validate_vector_index(vector.as_value_ref(), index.as_value_ref())?;
        let value_ty = require_type(
            unsafe { LLVMTypeOf(element.as_value_ref()) },
            "vector element",
        )?;
        if element_ty != value_ty {
            return Err(LlvmError::error(
                "LLVM vector insertion value type does not match the vector element type",
            ));
        }
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

    /// Rearranges lanes from two vectors according to `mask`.
    pub fn build_shuffle_vector(
        &self,
        lhs: VectorValue<'ctx>,
        rhs: VectorValue<'ctx>,
        mask: VectorValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        validate_shuffle_types(lhs.as_value_ref(), rhs.as_value_ref(), mask.as_value_ref())?;
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

    /// Bitcasts a value without changing its bit representation.
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

    /// Casts between opaque pointer types/address spaces.
    pub fn build_pointer_cast(
        &self,
        value: PointerValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            LLVMBuildPointerCast(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        };
        Ok(PointerValue::new(require_value(value, "pointer cast")?))
    }

    /// Converts a pointer to an integer of `target` width.
    pub fn build_ptr_to_int(
        &self,
        value: PointerValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            LLVMBuildPtrToInt(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        };
        Ok(IntValue::new(require_value(
            value,
            "pointer-to-integer cast",
        )?))
    }

    /// Converts an integer to an opaque pointer.
    pub fn build_int_to_ptr(
        &self,
        value: IntValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            LLVMBuildIntToPtr(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        };
        Ok(PointerValue::new(require_value(
            value,
            "integer-to-pointer cast",
        )?))
    }

    /// Zero-extends a scalar integer.
    pub fn build_int_z_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(
            self.raw,
            LLVMBuildZExt,
            value,
            target,
            IntegerCastKind::Extend,
            name,
        )
    }

    /// Sign-extends a scalar integer.
    pub fn build_int_s_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(
            self.raw,
            LLVMBuildSExt,
            value,
            target,
            IntegerCastKind::Extend,
            name,
        )
    }

    /// Truncates a scalar integer to a narrower width.
    pub fn build_int_truncate(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int(
            self.raw,
            LLVMBuildTrunc,
            value,
            target,
            IntegerCastKind::Truncate,
            name,
        )
    }

    /// Zero-extends integer scalars or vectors.
    pub fn build_basic_int_z_extend(
        &self,
        value: BasicValueEnum<'ctx>,
        target: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        cast_basic(
            self.raw,
            LLVMBuildZExt,
            value,
            target,
            IntegerCastKind::Extend,
            name,
        )
    }

    /// Sign-extends integer scalars or vectors.
    pub fn build_basic_int_s_extend(
        &self,
        value: BasicValueEnum<'ctx>,
        target: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        cast_basic(
            self.raw,
            LLVMBuildSExt,
            value,
            target,
            IntegerCastKind::Extend,
            name,
        )
    }

    /// Truncates integer scalars or vectors.
    pub fn build_basic_int_truncate(
        &self,
        value: BasicValueEnum<'ctx>,
        target: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicValueEnum<'ctx>> {
        cast_basic(
            self.raw,
            LLVMBuildTrunc,
            value,
            target,
            IntegerCastKind::Truncate,
            name,
        )
    }

    /// Converts a signed scalar integer to floating point.
    pub fn build_signed_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        cast_float(self.raw, LLVMBuildSIToFP, value, target, name)
    }

    /// Converts an unsigned scalar integer to floating point.
    pub fn build_unsigned_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> LlvmResult<FloatValue<'ctx>> {
        cast_float(self.raw, LLVMBuildUIToFP, value, target, name)
    }

    /// Converts floating point to a signed scalar integer.
    pub fn build_float_to_signed_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int_from_float(self.raw, LLVMBuildFPToSI, value, target, name)
    }

    /// Converts floating point to an unsigned scalar integer.
    pub fn build_float_to_unsigned_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> LlvmResult<IntValue<'ctx>> {
        cast_int_from_float(self.raw, LLVMBuildFPToUI, value, target, name)
    }

    /// Converts between floating-point widths.
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

/// Converts an LLVM value-producing call into the wrapper's fallible result.
///
/// The C API uses a null `LLVMValueRef` to report builder failure. Check it
/// immediately, before a typed handle constructor or a follow-up API call can
/// dereference the invalid result.
fn require_value(raw: LLVMValueRef, operation: &str) -> LlvmResult<LLVMValueRef> {
    if raw.is_null() {
        Err(super::LlvmError::error(format!(
            "LLVM returned a null value while building {operation}"
        )))
    } else {
        Ok(raw)
    }
}

fn require_type(raw: LLVMTypeRef, operation: &str) -> LlvmResult<LLVMTypeRef> {
    if raw.is_null() {
        Err(LlvmError::error(format!(
            "LLVM returned a null type while checking {operation}"
        )))
    } else {
        Ok(raw)
    }
}

/// Returns the physical type of one top-level aggregate field.
///
/// LLVM's aggregate builders otherwise accept an unchecked `u32` index and
/// rely on the verifier (or a null result) to report malformed IR. Validate it
/// before entering the FFI so the safe wrapper preserves its `LlvmResult`
/// contract and never asks LLVM to construct an invalid instruction.
fn aggregate_element_type(aggregate: LLVMValueRef, index: u32) -> LlvmResult<LLVMTypeRef> {
    let aggregate_ty = require_type(unsafe { LLVMTypeOf(aggregate) }, "aggregate value")?;
    let field_ty = match unsafe { LLVMGetTypeKind(aggregate_ty) } {
        LLVMTypeKind::LLVMStructTypeKind => {
            let count = unsafe { LLVMCountStructElementTypes(aggregate_ty) };
            if u64::from(index) >= u64::from(count) {
                return Err(LlvmError::error(format!(
                    "aggregate field index {index} is out of bounds for struct with {count} fields"
                )));
            }
            unsafe { LLVMStructGetTypeAtIndex(aggregate_ty, index) }
        }
        LLVMTypeKind::LLVMArrayTypeKind => {
            let count = unsafe { LLVMGetArrayLength2(aggregate_ty) };
            if u64::from(index) >= count {
                return Err(LlvmError::error(format!(
                    "aggregate field index {index} is out of bounds for array with {count} elements"
                )));
            }
            unsafe { LLVMGetElementType(aggregate_ty) }
        }
        kind => {
            return Err(LlvmError::error(format!(
                "LLVM aggregate operation received non-aggregate type {kind:?}"
            )));
        }
    };
    require_type(field_ty, "aggregate field")
}

fn validate_select_types(
    cond: LLVMValueRef,
    on_true: LLVMValueRef,
    on_false: LLVMValueRef,
) -> LlvmResult<()> {
    let cond_ty = require_type(unsafe { LLVMTypeOf(cond) }, "select condition")?;
    let true_ty = require_type(unsafe { LLVMTypeOf(on_true) }, "select true value")?;
    let false_ty = require_type(unsafe { LLVMTypeOf(on_false) }, "select false value")?;
    if true_ty != false_ty {
        return Err(LlvmError::error(
            "LLVM select arms must have identical types",
        ));
    }
    match unsafe { LLVMGetTypeKind(cond_ty) } {
        LLVMTypeKind::LLVMIntegerTypeKind => {
            if unsafe { LLVMGetIntTypeWidth(cond_ty) } != 1 {
                return Err(LlvmError::error(
                    "LLVM scalar select condition must have i1 type",
                ));
            }
        }
        LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind => {
            let cond_element =
                require_type(unsafe { LLVMGetElementType(cond_ty) }, "select condition")?;
            if unsafe { LLVMGetTypeKind(cond_element) } != LLVMTypeKind::LLVMIntegerTypeKind
                || unsafe { LLVMGetIntTypeWidth(cond_element) } != 1
            {
                return Err(LlvmError::error(
                    "LLVM vector select condition must contain i1 lanes",
                ));
            }
            if !matches!(
                unsafe { LLVMGetTypeKind(true_ty) },
                LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind
            ) || unsafe { LLVMGetTypeKind(true_ty) } != unsafe { LLVMGetTypeKind(cond_ty) }
                || unsafe { LLVMGetVectorSize(true_ty) } != unsafe { LLVMGetVectorSize(cond_ty) }
            {
                return Err(LlvmError::error(
                    "LLVM vector select condition and arms must have matching vector shape",
                ));
            }
        }
        kind => {
            return Err(LlvmError::error(format!(
                "LLVM select condition must be i1 or an i1 vector, got {kind:?}"
            )));
        }
    }
    Ok(())
}

fn validate_vector_index(vector: LLVMValueRef, index: LLVMValueRef) -> LlvmResult<LLVMTypeRef> {
    let vector_ty = require_type(unsafe { LLVMTypeOf(vector) }, "vector value")?;
    if !matches!(
        unsafe { LLVMGetTypeKind(vector_ty) },
        LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind
    ) {
        return Err(LlvmError::error(
            "LLVM vector operation received a non-vector value",
        ));
    }
    let index_ty = require_type(unsafe { LLVMTypeOf(index) }, "vector index")?;
    if unsafe { LLVMGetTypeKind(index_ty) } != LLVMTypeKind::LLVMIntegerTypeKind {
        return Err(LlvmError::error(
            "LLVM vector index must have an integer type",
        ));
    }
    require_type(unsafe { LLVMGetElementType(vector_ty) }, "vector element")
}

fn validate_shuffle_types(
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    mask: LLVMValueRef,
) -> LlvmResult<()> {
    let lhs_ty = require_type(unsafe { LLVMTypeOf(lhs) }, "shuffle left vector")?;
    let rhs_ty = require_type(unsafe { LLVMTypeOf(rhs) }, "shuffle right vector")?;
    if !matches!(
        unsafe { LLVMGetTypeKind(lhs_ty) },
        LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind
    ) || lhs_ty != rhs_ty
    {
        return Err(LlvmError::error(
            "LLVM shuffle inputs must have identical vector types",
        ));
    }
    let mask_ty = require_type(unsafe { LLVMTypeOf(mask) }, "shuffle mask")?;
    if !matches!(
        unsafe { LLVMGetTypeKind(mask_ty) },
        LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind
    ) {
        return Err(LlvmError::error(
            "LLVM shuffle mask must be an integer vector",
        ));
    }
    let mask_element = require_type(unsafe { LLVMGetElementType(mask_ty) }, "shuffle mask")?;
    if unsafe { LLVMGetTypeKind(mask_element) } != LLVMTypeKind::LLVMIntegerTypeKind
        || unsafe { LLVMGetIntTypeWidth(mask_element) } != 32
    {
        return Err(LlvmError::error(
            "LLVM shuffle mask lanes must have i32 type",
        ));
    }
    Ok(())
}

fn build_int_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: IntValue<'ctx>,
    rhs: IntValue<'ctx>,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    let name = to_c_string(name)?;
    let value = unsafe {
        f(
            builder,
            lhs.as_value_ref(),
            rhs.as_value_ref(),
            name.as_ptr(),
        )
    };
    Ok(IntValue::new(require_value(value, "integer operation")?))
}

#[derive(Clone, Copy)]
enum IntegerCastKind {
    Extend,
    Truncate,
}

fn validate_integer_cast(
    value_ty: LLVMTypeRef,
    target_ty: LLVMTypeRef,
    kind: IntegerCastKind,
) -> LlvmResult<()> {
    let value_ty = require_type(value_ty, "integer cast source")?;
    let target_ty = require_type(target_ty, "integer cast target")?;
    let value_kind = unsafe { LLVMGetTypeKind(value_ty) };
    let target_kind = unsafe { LLVMGetTypeKind(target_ty) };
    let is_integer = |kind| {
        matches!(
            kind,
            LLVMTypeKind::LLVMIntegerTypeKind
                | LLVMTypeKind::LLVMVectorTypeKind
                | LLVMTypeKind::LLVMScalableVectorTypeKind
        )
    };
    if !is_integer(value_kind) || !is_integer(target_kind) {
        return Err(LlvmError::error(
            "LLVM integer cast requires integer scalar or vector types",
        ));
    }
    let source_bits = integer_cast_shape(value_ty)?;
    let target_bits = integer_cast_shape(target_ty)?;
    if source_bits.0 != target_bits.0 || source_bits.1 != target_bits.1 {
        return Err(LlvmError::error(
            "LLVM integer cast source and target must have matching vector shape",
        ));
    }
    match kind {
        IntegerCastKind::Extend if target_bits.2 <= source_bits.2 => Err(LlvmError::error(
            "LLVM integer extension target must be wider than its source",
        )),
        IntegerCastKind::Truncate if target_bits.2 >= source_bits.2 => Err(LlvmError::error(
            "LLVM integer truncation target must be narrower than its source",
        )),
        _ => Ok(()),
    }
}

/// Returns `(kind, lane count, lane width)` for an integer scalar/vector.
fn integer_cast_shape(ty: LLVMTypeRef) -> LlvmResult<(LLVMTypeKind, u32, u32)> {
    match unsafe { LLVMGetTypeKind(ty) } {
        LLVMTypeKind::LLVMIntegerTypeKind => Ok((LLVMTypeKind::LLVMIntegerTypeKind, 1, unsafe {
            LLVMGetIntTypeWidth(ty)
        })),
        LLVMTypeKind::LLVMVectorTypeKind | LLVMTypeKind::LLVMScalableVectorTypeKind => {
            let element =
                require_type(unsafe { LLVMGetElementType(ty) }, "integer vector element")?;
            if unsafe { LLVMGetTypeKind(element) } != LLVMTypeKind::LLVMIntegerTypeKind {
                return Err(LlvmError::error(
                    "LLVM integer vector cast requires integer lanes",
                ));
            }
            Ok((
                unsafe { LLVMGetTypeKind(ty) },
                unsafe { LLVMGetVectorSize(ty) },
                unsafe { LLVMGetIntTypeWidth(element) },
            ))
        }
        _ => Err(LlvmError::error("LLVM value is not an integer type")),
    }
}

fn build_float_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: FloatValue<'ctx>,
    rhs: FloatValue<'ctx>,
    name: &str,
) -> LlvmResult<FloatValue<'ctx>> {
    let name = to_c_string(name)?;
    let value = unsafe {
        f(
            builder,
            lhs.as_value_ref(),
            rhs.as_value_ref(),
            name.as_ptr(),
        )
    };
    Ok(FloatValue::new(require_value(value, "floating operation")?))
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
    kind: IntegerCastKind,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    validate_integer_cast(
        unsafe { LLVMTypeOf(value.as_value_ref()) },
        target.as_type_ref(),
        kind,
    )?;
    let name = to_c_string(name)?;
    let value = unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    };
    Ok(IntValue::new(require_value(value, "integer cast")?))
}

fn cast_basic<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: BasicValueEnum<'ctx>,
    target: BasicTypeEnum<'ctx>,
    kind: IntegerCastKind,
    name: &str,
) -> LlvmResult<BasicValueEnum<'ctx>> {
    validate_integer_cast(
        unsafe { LLVMTypeOf(value.as_value_ref()) },
        target.as_type_ref(),
        kind,
    )?;
    let name = to_c_string(name)?;
    BasicValueEnum::new(unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    })
}

fn cast_float<'ctx, V: AsValueRef>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: V,
    target: FloatType<'ctx>,
    name: &str,
) -> LlvmResult<FloatValue<'ctx>> {
    let name = to_c_string(name)?;
    let value = unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    };
    Ok(FloatValue::new(require_value(value, "floating cast")?))
}

fn cast_int_from_float<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: FloatValue<'ctx>,
    target: IntType<'ctx>,
    name: &str,
) -> LlvmResult<IntValue<'ctx>> {
    let name = to_c_string(name)?;
    let value = unsafe {
        f(
            builder,
            value.as_value_ref(),
            target.as_type_ref(),
            name.as_ptr(),
        )
    };
    Ok(IntValue::new(require_value(
        value,
        "floating-to-integer cast",
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_null_builder_results_before_typed_handle_construction() {
        let error = require_value(std::ptr::null_mut(), "test instruction")
            .expect_err("null builder result");

        assert_eq!(
            error,
            super::super::LlvmError::Error(
                "LLVM returned a null value while building test instruction".to_string()
            )
        );
    }

    #[test]
    fn rejects_out_of_bounds_aggregate_index_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("aggregate-index").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);
        let aggregate_ty = context.struct_type(&[context.i32_type().into()], false);

        let error = builder
            .build_extract_value(aggregate_ty.const_zero(), 1, "invalid")
            .expect_err("out-of-bounds aggregate index");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("field index 1 is out of bounds")
        ));
    }

    #[test]
    fn rejects_aggregate_insert_type_mismatch_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("aggregate-type").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);
        let aggregate_ty = context.struct_type(&[context.i32_type().into()], false);

        let error = builder
            .build_insert_value(
                aggregate_ty.const_zero(),
                context.i64_type().const_zero(),
                0,
                "invalid",
            )
            .expect_err("aggregate insertion type mismatch");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("insertion value type does not match")
        ));
    }

    #[test]
    fn rejects_select_arm_type_mismatch_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("select-type").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);

        let error = builder
            .build_select(
                context.bool_type().const_zero().into(),
                context.i32_type().const_zero().into(),
                context.i64_type().const_zero().into(),
                "invalid",
            )
            .expect_err("select arm type mismatch");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("select arms must have identical types")
        ));
    }

    #[test]
    fn accepts_scalar_select_for_vector_values() {
        let context = Context::create();
        let module = context.create_module("select-vector").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);
        let vector = BasicTypeEnum::from(context.i32_type())
            .vector_type(2)
            .const_zero()
            .unwrap();

        let value = builder
            .build_select(
                context.bool_type().const_zero().into(),
                vector,
                vector,
                "valid",
            )
            .unwrap();
        assert!(value.is_vector_value());
    }

    #[test]
    fn rejects_vector_insert_element_type_mismatch_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("vector-type").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);
        let vector = BasicTypeEnum::from(context.i32_type())
            .vector_type(2)
            .const_zero()
            .unwrap()
            .into_vector_value()
            .unwrap();
        let index = context.i32_type().const_zero();

        let error = builder
            .build_insert_element(
                vector,
                context.i64_type().const_zero().into(),
                index,
                "invalid",
            )
            .expect_err("vector insertion type mismatch");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("vector insertion value type does not match")
        ));
    }

    #[test]
    fn rejects_shuffle_mask_with_non_i32_lanes_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("shuffle-mask").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);
        let vector = BasicTypeEnum::from(context.i32_type())
            .vector_type(2)
            .const_zero()
            .unwrap()
            .into_vector_value()
            .unwrap();
        let mask = BasicTypeEnum::from(context.i64_type())
            .vector_type(2)
            .const_zero()
            .unwrap()
            .into_vector_value()
            .unwrap();

        let error = builder
            .build_shuffle_vector(vector, vector, mask, "invalid")
            .expect_err("shuffle mask lane type mismatch");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("shuffle mask lanes must have i32 type")
        ));
    }

    #[test]
    fn rejects_integer_extension_without_a_wider_target() {
        let context = Context::create();
        let module = context.create_module("integer-extend").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);

        let error = builder
            .build_int_z_extend(
                context.i32_type().const_zero(),
                context.i32_type(),
                "invalid",
            )
            .expect_err("same-width integer extension");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("extension target must be wider")
        ));
    }

    #[test]
    fn rejects_integer_truncation_to_a_wider_target() {
        let context = Context::create();
        let module = context.create_module("integer-truncate").unwrap();
        let function = module
            .add_function("test", context.void_type().fn_type(&[], false), None)
            .unwrap();
        let block = context.append_basic_block(function, "entry").unwrap();
        let builder = context.create_builder();
        builder.position_at_end(block);

        let error = builder
            .build_int_truncate(
                context.i32_type().const_zero(),
                context.i64_type(),
                "invalid",
            )
            .expect_err("wider integer truncation target");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("truncation target must be narrower")
        ));
    }
}
