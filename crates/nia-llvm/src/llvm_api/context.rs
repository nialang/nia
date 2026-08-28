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
    LLVMInt64TypeInContext, LLVMInt128TypeInContext, LLVMIntTypeInContext,
    LLVMModuleCreateWithNameInContext, LLVMPointerTypeInContext, LLVMStructCreateNamed,
    LLVMStructTypeInContext, LLVMVoidTypeInContext,
};
use llvm_sys::prelude::LLVMContextRef;

use super::{
    AddressSpace, ArrayValue, AsTypeRef, AsValueRef, Attribute, BasicBlock, BasicTypeEnum, Builder,
    FloatType, FunctionType, FunctionValue, InlineAsmDialect, IntType, LlvmError, LlvmResult,
    Module, PointerType, PointerValue, StructType, VoidType, bool_to_llvm, checked_u32_count,
    require_value, to_c_string,
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
    pub fn create() -> LlvmResult<Self> {
        let raw = unsafe { LLVMContextCreate() };
        Ok(Self {
            raw: require_handle(raw, "context")?,
        })
    }

    /// Creates an instruction builder owned by this context.
    pub fn create_builder<'ctx>(&'ctx self) -> LlvmResult<Builder<'ctx>> {
        let raw = unsafe { LLVMCreateBuilderInContext(self.raw) };
        Ok(Builder::new(require_handle(raw, "builder")?))
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
        if !is_bitcode_header(bitcode) {
            return Err(LlvmError::error(
                "LLVM bitcode input has an invalid or truncated header",
            ));
        }
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
    pub fn create_enum_attribute<'ctx>(
        &'ctx self,
        kind_id: u32,
        val: u64,
    ) -> LlvmResult<Attribute<'ctx>> {
        Attribute::new(unsafe { LLVMCreateEnumAttribute(self.raw, kind_id, val) })
    }

    /// Creates a string attribute and rejects a null LLVM result.
    pub fn create_string_attribute<'ctx>(
        &'ctx self,
        key: &str,
        value: &str,
    ) -> LlvmResult<Attribute<'ctx>> {
        let key_len = checked_u32_count(key.len(), "LLVM string attribute key is too long")?;
        let value_len = checked_u32_count(value.len(), "LLVM string attribute value is too long")?;
        Attribute::new(unsafe {
            LLVMCreateStringAttribute(
                self.raw,
                key.as_ptr() as *const _,
                key_len,
                value.as_ptr() as *const _,
                value_len,
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
        IntType::new(unsafe { LLVMInt128TypeInContext(self.raw) })
    }

    /// Returns an integer type with exactly `bits` bits.
    pub fn custom_width_int_type<'ctx>(&'ctx self, bits: u32) -> LlvmResult<IntType<'ctx>> {
        if bits == 0 {
            return Err(LlvmError::error(
                "LLVM custom integer type requires a non-zero bit width",
            ));
        }
        let raw = unsafe { LLVMIntTypeInContext(self.raw, bits) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null custom integer type"));
        }
        Ok(IntType::new(raw))
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
    ) -> LlvmResult<StructType<'ctx>> {
        let field_count = checked_u32_count(fields.len(), "LLVM struct type has too many fields")?;
        let mut fields = fields
            .iter()
            .map(|field| field.as_type_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMStructTypeInContext(
                self.raw,
                fields.as_mut_ptr(),
                field_count,
                bool_to_llvm(packed),
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null struct type"));
        }
        Ok(StructType::new(raw))
    }

    /// Creates a named opaque struct whose body may be supplied later.
    pub fn opaque_struct_type<'ctx>(&'ctx self, name: &str) -> LlvmResult<StructType<'ctx>> {
        let name = to_c_string(name)?;
        let raw = unsafe { LLVMStructCreateNamed(self.raw, name.as_ptr()) };
        Ok(StructType::new(require_handle(raw, "opaque struct type")?))
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
    ) -> LlvmResult<ArrayValue<'ctx>> {
        let raw = unsafe {
            LLVMConstStringInContext2(
                self.raw,
                bytes.as_ptr() as *const _,
                bytes.len(),
                bool_to_llvm(dont_null_terminate),
            )
        };
        Ok(ArrayValue::new(require_value(raw, "constant string")?))
    }
}

/// Returns whether `bitcode` starts with one of LLVM's two documented bitcode
/// signatures: raw bitcode (`BC C0 DE`) or the legacy wrapper (`DE C0 17 0B`).
/// This cheap check keeps obviously malformed input away from LLVM's parser,
/// which may report a fatal diagnostic for buffers too short to contain its
/// header instead of returning through the C API.
fn is_bitcode_header(bitcode: &[u8]) -> bool {
    bitcode.get(..4).is_some_and(|header| {
        header == [b'B', b'C', 0xc0, 0xde] || header == [0xde, 0xc0, 0x17, 0x0b]
    })
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

    #[test]
    fn rejects_null_context_before_wrapper_construction() {
        let error = require_handle::<llvm_sys::LLVMContext>(std::ptr::null_mut(), "context")
            .expect_err("null context handle");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null context handle".to_string())
        );
    }

    #[test]
    fn rejects_null_opaque_struct_type_before_wrapper_construction() {
        let error =
            require_handle::<llvm_sys::LLVMType>(std::ptr::null_mut(), "opaque struct type")
                .expect_err("null opaque struct type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null opaque struct type handle".to_string())
        );
    }

    #[test]
    fn rejects_null_builder_before_wrapper_construction() {
        let error = require_handle::<llvm_sys::LLVMBuilder>(std::ptr::null_mut(), "builder")
            .expect_err("null builder handle");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null builder handle".to_string())
        );
    }

    #[test]
    fn rejects_zero_width_custom_integer_types() {
        let context = Context::create().unwrap();

        let error = context
            .custom_width_int_type(0)
            .expect_err("zero-width integer type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM custom integer type requires a non-zero bit width".to_string())
        );
    }

    #[test]
    fn rejects_empty_or_malformed_bitcode_before_llvm_parser() {
        let context = Context::create().unwrap();

        for input in [&[][..], &[b'B', b'C', 0xc0][..], &[0u8, 1, 2, 3][..]] {
            let error = context
                .parse_bitcode_module("malformed", input)
                .expect_err("malformed bitcode");

            assert_eq!(
                error,
                LlvmError::Error(
                    "LLVM bitcode input has an invalid or truncated header".to_string()
                )
            );
        }
    }

    #[test]
    fn recognizes_raw_and_wrapped_bitcode_headers() {
        assert!(is_bitcode_header(&[b'B', b'C', 0xc0, 0xde, 0]));
        assert!(is_bitcode_header(&[0xde, 0xc0, 0x17, 0x0b, 0]));
        assert!(!is_bitcode_header(&[b'B', b'C', 0xc0]));
        assert!(!is_bitcode_header(&[0, 1, 2, 3]));
    }

    #[test]
    fn parses_bitcode_emitted_by_module_round_trip() {
        let context = Context::create().unwrap();
        let source = context.create_module("round-trip").unwrap();
        let bitcode = source.bitcode().unwrap();

        let parsed = context
            .parse_bitcode_module("round-trip-copy", &bitcode)
            .expect("LLVM should parse its own bitcode");
        parsed.verify().expect("round-tripped module should verify");
        assert!(!parsed.ir_string().unwrap().is_empty());
    }
}
