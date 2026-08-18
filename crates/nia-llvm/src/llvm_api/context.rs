// SPDX-License-Identifier: GPL-3.0-or-later
//! LLVM context wrapper.
//!
//! The context owns LLVM's top-level allocation arena and creates modules,
//! builders, memory buffers, attributes, primitive types, constants, and basic
//! blocks that must not outlive it.

use llvm_sys::bit_reader::LLVMParseBitcodeInContext2;
use llvm_sys::core::{
    LLVMAppendBasicBlockInContext, LLVMConstStringInContext2, LLVMContextCreate,
    LLVMContextDispose, LLVMCreateBuilderInContext, LLVMCreateEnumAttribute,
    LLVMCreateMemoryBufferWithMemoryRangeCopy, LLVMCreateStringAttribute, LLVMDisposeMemoryBuffer,
    LLVMDisposeModule, LLVMDoubleTypeInContext, LLVMFloatTypeInContext, LLVMGetInlineAsm,
    LLVMInt1TypeInContext, LLVMInt8TypeInContext, LLVMInt16TypeInContext, LLVMInt32TypeInContext,
    LLVMInt64TypeInContext, LLVMIntTypeInContext, LLVMModuleCreateWithNameInContext,
    LLVMPointerTypeInContext, LLVMStructCreateNamed, LLVMStructTypeInContext,
    LLVMVoidTypeInContext,
};
use llvm_sys::prelude::LLVMContextRef;

use super::{
    AddressSpace, ArrayValue, AsTypeRef, AsValueRef, Attribute, BasicBlock, BasicTypeEnum, Builder,
    FloatType, FunctionType, FunctionValue, InlineAsmDialect, IntType, LlvmError, LlvmResult,
    Module, PointerType, PointerValue, StructType, VoidType, bool_to_llvm, to_c_string,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// LLVM inline-assembly flags passed without reinterpretation by codegen.
pub struct InlineAsmOptions {
    /// Whether the assembly has side effects beyond its explicit outputs.
    pub sideeffects: bool,
    /// Whether LLVM must realign the stack before entering the assembly.
    pub alignstack: bool,
    /// Assembly syntax; omitted values use LLVM's AT&T dialect.
    pub dialect: Option<InlineAsmDialect>,
    /// Whether control may unwind out of the assembly.
    pub can_throw: bool,
}

#[derive(Debug, PartialEq, Eq)]
/// Owned LLVM context and lifetime root for all typed wrapper handles.
pub struct Context {
    pub(super) raw: LLVMContextRef,
}

impl Context {
    /// Creates a fresh LLVM context.
    ///
    /// LLVM treats context allocation failure as unrecoverable; consequently
    /// this is the one top-level wrapper constructor that asserts non-null.
    pub fn create() -> Self {
        let raw = unsafe { LLVMContextCreate() };
        assert!(!raw.is_null());
        Self { raw }
    }

    /// Creates an instruction builder owned by this context.
    ///
    /// LLVM documents context and builder allocation as infallible. Keep the
    /// assertion here because there is no useful recoverable error contract
    /// for a context that cannot allocate its own builder.
    pub fn create_builder<'ctx>(&'ctx self) -> Builder<'ctx> {
        let raw = unsafe { LLVMCreateBuilderInContext(self.raw) };
        assert!(!raw.is_null());
        Builder::new(raw)
    }

    /// Creates a module, surfacing invalid names or a null LLVM result.
    pub fn create_module<'ctx>(&'ctx self, name: &str) -> LlvmResult<Module<'ctx>> {
        let name = to_c_string(name)?;
        let raw = unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), self.raw) };
        let raw = require_handle(raw, "module")?;
        Ok(Module::new(raw))
    }

    /// Parses serialized bitcode into a module owned by this context.
    pub fn parse_bitcode_module<'ctx>(
        &'ctx self,
        name: &str,
        bitcode: &[u8],
    ) -> LlvmResult<Module<'ctx>> {
        let name = to_c_string(name)?;
        let buffer = unsafe {
            LLVMCreateMemoryBufferWithMemoryRangeCopy(
                bitcode.as_ptr() as *const _,
                bitcode.len(),
                name.as_ptr(),
            )
        };
        if buffer.is_null() {
            return Err(LlvmError::error(
                "LLVM failed to create a bitcode memory buffer",
            ));
        }

        let mut raw_module = std::ptr::null_mut();
        let failed = unsafe { LLVMParseBitcodeInContext2(self.raw, buffer, &mut raw_module) } != 0;
        unsafe { LLVMDisposeMemoryBuffer(buffer) };
        if failed || raw_module.is_null() {
            // LLVM normally leaves `raw_module` null on failure. Dispose a
            // partially produced module as well so a backend/FFI change cannot
            // turn a parse error into a leaked LLVM context allocation.
            if !raw_module.is_null() {
                unsafe { LLVMDisposeModule(raw_module) };
            }
            return Err(LlvmError::error(
                "LLVM failed to parse a serialized bitcode module",
            ));
        }

        Ok(Module::new(raw_module))
    }

    /// Creates an LLVM inline-assembly function pointer.
    ///
    /// LLVM may reject the function type or assembly/constraint combination
    /// by returning a null value. Surface that failure as [`LlvmError`] so a
    /// malformed backend contract becomes a diagnostic instead of triggering
    /// the non-null handle assertions used by value wrappers.
    pub fn create_inline_asm<'ctx>(
        &'ctx self,
        ty: FunctionType<'ctx>,
        mut assembly: String,
        mut constraints: String,
        options: InlineAsmOptions,
    ) -> LlvmResult<PointerValue<'ctx>> {
        let raw = unsafe {
            LLVMGetInlineAsm(
                ty.as_type_ref(),
                assembly.as_mut_ptr() as *mut _,
                assembly.len(),
                constraints.as_mut_ptr() as *mut _,
                constraints.len(),
                bool_to_llvm(options.sideeffects),
                bool_to_llvm(options.alignstack),
                options.dialect.unwrap_or(InlineAsmDialect::ATT).into(),
                bool_to_llvm(options.can_throw),
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error(
                "LLVM returned a null inline-assembly value",
            ));
        }
        Ok(PointerValue::new(raw))
    }

    /// Creates an enum attribute and rejects a null LLVM result.
    pub fn create_enum_attribute(&self, kind_id: u32, val: u64) -> LlvmResult<Attribute> {
        Attribute::new(unsafe { LLVMCreateEnumAttribute(self.raw, kind_id, val) })
    }

    /// Creates a string attribute and rejects a null LLVM result.
    pub fn create_string_attribute(&self, key: &str, value: &str) -> LlvmResult<Attribute> {
        Attribute::new(unsafe {
            LLVMCreateStringAttribute(
                self.raw,
                key.as_ptr() as *const _,
                key.len() as u32,
                value.as_ptr() as *const _,
                value.len() as u32,
            )
        })
    }

    /// Returns this context's `void` type.
    pub fn void_type<'ctx>(&'ctx self) -> VoidType<'ctx> {
        VoidType::new(unsafe { LLVMVoidTypeInContext(self.raw) })
    }

    /// Returns the one-bit integer type used for booleans.
    pub fn bool_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt1TypeInContext(self.raw) })
    }

    /// Returns this context's 8-bit integer type.
    pub fn i8_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt8TypeInContext(self.raw) })
    }

    /// Returns this context's 16-bit integer type.
    pub fn i16_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt16TypeInContext(self.raw) })
    }

    /// Returns this context's 32-bit integer type.
    pub fn i32_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt32TypeInContext(self.raw) })
    }

    /// Returns this context's 64-bit integer type.
    pub fn i64_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt64TypeInContext(self.raw) })
    }

    /// Returns this context's 128-bit integer type.
    pub fn i128_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        self.custom_width_int_type(128)
    }

    /// Returns an integer type with exactly `bits` bits.
    pub fn custom_width_int_type<'ctx>(&'ctx self, bits: u32) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMIntTypeInContext(self.raw, bits) })
    }

    /// Returns this context's IEEE-754 single-precision type.
    pub fn f32_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMFloatTypeInContext(self.raw) })
    }

    /// Returns this context's IEEE-754 double-precision type.
    pub fn f64_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMDoubleTypeInContext(self.raw) })
    }

    /// Returns an opaque pointer type in `address_space`.
    pub fn ptr_type<'ctx>(&'ctx self, address_space: AddressSpace) -> PointerType<'ctx> {
        PointerType::new(unsafe { LLVMPointerTypeInContext(self.raw, address_space.0) })
    }

    /// Creates a literal struct type with the supplied physical field order.
    pub fn struct_type<'ctx>(
        &'ctx self,
        fields: &[BasicTypeEnum<'ctx>],
        packed: bool,
    ) -> StructType<'ctx> {
        let mut fields = fields
            .iter()
            .map(|field| field.as_type_ref())
            .collect::<Vec<_>>();
        StructType::new(unsafe {
            LLVMStructTypeInContext(
                self.raw,
                fields.as_mut_ptr(),
                fields.len() as u32,
                bool_to_llvm(packed),
            )
        })
    }

    /// Creates a named opaque struct whose body may be supplied later.
    pub fn opaque_struct_type<'ctx>(&'ctx self, name: &str) -> LlvmResult<StructType<'ctx>> {
        let name = to_c_string(name)?;
        Ok(StructType::new(unsafe {
            LLVMStructCreateNamed(self.raw, name.as_ptr())
        }))
    }

    /// Appends a named basic block to `function`.
    pub fn append_basic_block<'ctx>(
        &'ctx self,
        function: FunctionValue<'ctx>,
        name: &str,
    ) -> LlvmResult<BasicBlock<'ctx>> {
        let name = to_c_string(name)?;
        let raw = unsafe {
            LLVMAppendBasicBlockInContext(self.raw, function.as_value_ref(), name.as_ptr())
        };
        let raw = require_handle(raw, "basic block")?;
        Ok(BasicBlock::new(raw))
    }

    /// Creates an LLVM byte-array constant from arbitrary bytes.
    pub fn const_string<'ctx>(
        &'ctx self,
        bytes: &[u8],
        dont_null_terminate: bool,
    ) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe {
            LLVMConstStringInContext2(
                self.raw,
                bytes.as_ptr() as *const _,
                bytes.len(),
                bool_to_llvm(dont_null_terminate),
            )
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { LLVMContextDispose(self.raw) };
    }
}

/// Converts an LLVM allocation result into a recoverable error before a typed
/// wrapper's non-null constructor is called.
fn require_handle<T>(raw: *mut T, kind: &str) -> LlvmResult<*mut T> {
    if raw.is_null() {
        Err(LlvmError::error(format!(
            "LLVM returned a null {kind} handle"
        )))
    } else {
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_null_context_allocations_before_wrapper_construction() {
        let error = require_handle::<llvm_sys::LLVMModule>(std::ptr::null_mut(), "module")
            .expect_err("null module handle");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null module handle".to_string())
        );
    }
}
