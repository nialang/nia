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
    LLVMDoubleTypeInContext, LLVMFloatTypeInContext, LLVMGetInlineAsm, LLVMInt1TypeInContext,
    LLVMInt8TypeInContext, LLVMInt16TypeInContext, LLVMInt32TypeInContext, LLVMInt64TypeInContext,
    LLVMIntTypeInContext, LLVMModuleCreateWithNameInContext, LLVMPointerTypeInContext,
    LLVMStructCreateNamed, LLVMStructTypeInContext, LLVMVoidTypeInContext,
};
use llvm_sys::prelude::LLVMContextRef;

use super::{
    AddressSpace, ArrayValue, AsTypeRef, AsValueRef, Attribute, BasicBlock, BasicTypeEnum, Builder,
    FloatType, FunctionType, FunctionValue, InlineAsmDialect, IntType, Module, PointerType,
    PointerValue, StructType, VoidType, bool_to_llvm, to_c_string,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InlineAsmOptions {
    pub sideeffects: bool,
    pub alignstack: bool,
    pub dialect: Option<InlineAsmDialect>,
    pub can_throw: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Context {
    pub(super) raw: LLVMContextRef,
}

impl Context {
    pub fn create() -> Self {
        let raw = unsafe { LLVMContextCreate() };
        assert!(!raw.is_null());
        Self { raw }
    }

    pub fn create_builder<'ctx>(&'ctx self) -> Builder<'ctx> {
        let raw = unsafe { LLVMCreateBuilderInContext(self.raw) };
        assert!(!raw.is_null());
        Builder::new(raw)
    }

    pub fn create_module<'ctx>(&'ctx self, name: &str) -> Module<'ctx> {
        let name = to_c_string(name);
        let raw = unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), self.raw) };
        assert!(!raw.is_null());
        Module::new(raw)
    }

    pub fn parse_bitcode_module<'ctx>(
        &'ctx self,
        name: &str,
        bitcode: &[u8],
    ) -> Result<Module<'ctx>, String> {
        let name = to_c_string(name);
        let buffer = unsafe {
            LLVMCreateMemoryBufferWithMemoryRangeCopy(
                bitcode.as_ptr() as *const _,
                bitcode.len(),
                name.as_ptr(),
            )
        };
        if buffer.is_null() {
            return Err("LLVM failed to create a bitcode memory buffer".to_string());
        }

        let mut raw_module = std::ptr::null_mut();
        let failed = unsafe { LLVMParseBitcodeInContext2(self.raw, buffer, &mut raw_module) } != 0;
        unsafe { LLVMDisposeMemoryBuffer(buffer) };
        if failed || raw_module.is_null() {
            return Err("LLVM failed to parse a serialized bitcode module".to_string());
        }

        Ok(Module::new(raw_module))
    }

    pub fn create_inline_asm<'ctx>(
        &'ctx self,
        ty: FunctionType<'ctx>,
        mut assembly: String,
        mut constraints: String,
        options: InlineAsmOptions,
    ) -> PointerValue<'ctx> {
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
        PointerValue::new(raw)
    }

    pub fn create_enum_attribute(&self, kind_id: u32, val: u64) -> Attribute {
        Attribute {
            raw: unsafe { LLVMCreateEnumAttribute(self.raw, kind_id, val) },
        }
    }

    pub fn create_string_attribute(&self, key: &str, value: &str) -> Attribute {
        Attribute {
            raw: unsafe {
                LLVMCreateStringAttribute(
                    self.raw,
                    key.as_ptr() as *const _,
                    key.len() as u32,
                    value.as_ptr() as *const _,
                    value.len() as u32,
                )
            },
        }
    }

    pub fn void_type<'ctx>(&'ctx self) -> VoidType<'ctx> {
        VoidType::new(unsafe { LLVMVoidTypeInContext(self.raw) })
    }

    pub fn bool_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt1TypeInContext(self.raw) })
    }

    pub fn i8_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt8TypeInContext(self.raw) })
    }

    pub fn i16_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt16TypeInContext(self.raw) })
    }

    pub fn i32_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt32TypeInContext(self.raw) })
    }

    pub fn i64_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt64TypeInContext(self.raw) })
    }

    pub fn i128_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        self.custom_width_int_type(128)
    }

    pub fn custom_width_int_type<'ctx>(&'ctx self, bits: u32) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMIntTypeInContext(self.raw, bits) })
    }

    pub fn f32_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMFloatTypeInContext(self.raw) })
    }

    pub fn f64_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMDoubleTypeInContext(self.raw) })
    }

    pub fn ptr_type<'ctx>(&'ctx self, address_space: AddressSpace) -> PointerType<'ctx> {
        PointerType::new(unsafe { LLVMPointerTypeInContext(self.raw, address_space.0) })
    }

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

    pub fn opaque_struct_type<'ctx>(&'ctx self, name: &str) -> StructType<'ctx> {
        let name = to_c_string(name);
        StructType::new(unsafe { LLVMStructCreateNamed(self.raw, name.as_ptr()) })
    }

    pub fn append_basic_block<'ctx>(
        &'ctx self,
        function: FunctionValue<'ctx>,
        name: &str,
    ) -> BasicBlock<'ctx> {
        let name = to_c_string(name);
        BasicBlock::new(unsafe {
            LLVMAppendBasicBlockInContext(self.raw, function.as_value_ref(), name.as_ptr())
        })
    }

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
