// SPDX-License-Identifier: GPL-3.0-or-later
//! LLVM module wrapper.
//!
//! Modules own functions, globals, target/triple metadata, verification,
//! bitcode/object emission inputs, and module-level flags used by debug info and
//! target configuration.

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::bit_writer::LLVMWriteBitcodeToMemoryBuffer;
#[cfg(windows)]
use llvm_sys::core::LLVMPrintModuleToFile;
#[cfg(not(windows))]
use llvm_sys::core::LLVMPrintModuleToString;
use llvm_sys::core::{
    LLVMAddFunction, LLVMAddGlobal, LLVMAddGlobalInAddressSpace, LLVMDisposeMemoryBuffer,
    LLVMDisposeMessage, LLVMDisposeModule, LLVMGetBufferSize, LLVMGetBufferStart,
    LLVMGetFirstFunction, LLVMGetIntrinsicDeclaration, LLVMGetNamedFunction, LLVMGetNamedGlobal,
    LLVMGetNextFunction, LLVMSetLinkage,
};
use llvm_sys::linker::LLVMLinkModules2;
use llvm_sys::prelude::LLVMModuleRef;
use llvm_sys::target::LLVMSetModuleDataLayout;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr;
use std::slice;

use super::{
    AddressSpace, AsTypeRef, BasicTypeEnum, Context, FunctionType, FunctionValue, GlobalValue,
    Linkage, LlvmError, LlvmResult, to_c_string,
};

#[derive(Debug)]
/// Owned LLVM module tied to the lifetime of its originating context.
///
/// Dropping the wrapper disposes the module unless ownership was transferred
/// with [`Module::into_raw`].
pub struct Module<'ctx> {
    pub(super) raw: LLVMModuleRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Module<'ctx> {
    pub(super) fn new(raw: LLVMModuleRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Borrows the raw module handle without transferring ownership.
    pub fn as_mut_ptr(&self) -> LLVMModuleRef {
        self.raw
    }

    /// Transfers ownership of the raw module handle to the caller.
    ///
    /// The caller must eventually pass the handle to the appropriate LLVM
    /// disposer or to an API that explicitly consumes it.
    pub fn into_raw(mut self) -> LLVMModuleRef {
        let raw = self.raw;
        self.raw = ptr::null_mut();
        raw
    }

    /// Serializes the module to copied LLVM bitcode bytes.
    pub fn bitcode(&self) -> LlvmResult<Vec<u8>> {
        let buffer = unsafe { LLVMWriteBitcodeToMemoryBuffer(self.raw) };
        if buffer.is_null() {
            return Err(LlvmError::error(
                "LLVM failed to serialize a module to bitcode",
            ));
        }

        let bytes = unsafe {
            let start = LLVMGetBufferStart(buffer);
            let len = LLVMGetBufferSize(buffer);
            if len == 0 {
                Vec::new()
            } else if start.is_null() {
                LLVMDisposeMemoryBuffer(buffer);
                return Err(LlvmError::error(
                    "LLVM returned bitcode with a null buffer start",
                ));
            } else {
                // SAFETY: LLVM owns a live buffer with a non-null start and
                // reports its initialized byte length above.
                slice::from_raw_parts(start as *const u8, len).to_vec()
            }
        };
        unsafe { LLVMDisposeMemoryBuffer(buffer) };
        Ok(bytes)
    }

    /// Declares a function and applies an optional linkage.
    pub fn add_function(
        &self,
        name: &str,
        ty: FunctionType<'ctx>,
        linkage: Option<Linkage>,
    ) -> LlvmResult<FunctionValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMAddFunction(self.raw, name.as_ptr(), ty.as_type_ref()) };
        let value = require_handle(value, "function")?;
        let func = FunctionValue::new(value);
        if let Some(linkage) = linkage {
            unsafe { LLVMSetLinkage(value, linkage.into()) };
        }
        Ok(func)
    }

    /// Looks up a named function, returning `None` when it is absent.
    pub fn get_function(&self, name: &str) -> LlvmResult<Option<FunctionValue<'ctx>>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMGetNamedFunction(self.raw, name.as_ptr()) };
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(FunctionValue::new(value)))
        }
    }

    /// Returns the first function in LLVM module order.
    pub fn get_first_function(&self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetFirstFunction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    /// Declares a global value of `ty` in the requested address space.
    pub fn add_global(
        &self,
        ty: BasicTypeEnum<'ctx>,
        addr_space: Option<AddressSpace>,
        name: &str,
    ) -> LlvmResult<GlobalValue<'ctx>> {
        let name = to_c_string(name)?;
        let value = unsafe {
            match addr_space {
                Some(address_space) => LLVMAddGlobalInAddressSpace(
                    self.raw,
                    ty.as_type_ref(),
                    name.as_ptr(),
                    address_space.0,
                ),
                None => LLVMAddGlobal(self.raw, ty.as_type_ref(), name.as_ptr()),
            }
        };
        let value = require_handle(value, "global")?;
        Ok(GlobalValue::new(value))
    }

    /// Looks up a named global, returning `None` when it is absent.
    pub fn get_global(&self, name: &str) -> LlvmResult<Option<GlobalValue<'ctx>>> {
        let name = to_c_string(name)?;
        let value = unsafe { LLVMGetNamedGlobal(self.raw, name.as_ptr()) };
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(GlobalValue::new(value)))
        }
    }

    /// Sets the module target triple after validating it as a C string.
    pub fn set_triple(&self, triple: &str) -> LlvmResult<()> {
        let triple = to_c_string(triple)?;
        unsafe { llvm_sys::core::LLVMSetTarget(self.raw, triple.as_ptr()) };
        Ok(())
    }

    /// Links `source` into this module and transfers its ownership to LLVM.
    ///
    /// LLVM consumes the source module on both success and failure.
    pub fn link_in(&mut self, source: Module<'ctx>) -> LlvmResult<()> {
        let source = source.into_raw();
        let failed = unsafe { LLVMLinkModules2(self.raw, source) } != 0;
        if failed {
            return Err(LlvmError::error("LLVM failed to link modules"));
        }
        Ok(())
    }

    /// # Safety
    /// `target_data` must be a valid LLVM target-data handle for the intended
    /// target, and it must remain valid for the duration of this call.
    pub unsafe fn set_data_layout_from_target(
        &self,
        target_data: llvm_sys::target::LLVMTargetDataRef,
    ) {
        // SAFETY: The caller guarantees that `target_data` is a live LLVM
        // target-data handle. LLVM copies the layout onto the module during the
        // call, so no borrowed Rust data escapes this wrapper.
        unsafe { LLVMSetModuleDataLayout(self.raw, target_data) };
    }

    /// Runs LLVM's structural verifier and returns its owned diagnostic text.
    pub fn verify(&self) -> LlvmResult<()> {
        let mut message = ptr::null_mut();
        let failed = unsafe {
            LLVMVerifyModule(
                self.raw,
                LLVMVerifierFailureAction::LLVMReturnStatusAction,
                &mut message,
            )
        } != 0;
        if failed {
            let text = if message.is_null() {
                "LLVM verifier failed without an error message".to_string()
            } else {
                // SAFETY: LLVM returned a NUL-terminated diagnostic buffer.
                let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
                unsafe { LLVMDisposeMessage(message) };
                text
            };
            Err(LlvmError::error(text))
        } else {
            if !message.is_null() {
                unsafe { LLVMDisposeMessage(message) };
            }
            Ok(())
        }
    }

    /// Renders the module as textual LLVM IR.
    pub fn ir_string(&self) -> LlvmResult<String> {
        #[cfg(windows)]
        {
            self.ir_string_via_temp_file()
        }

        #[cfg(not(windows))]
        {
            self.ir_string_via_llvm_string()
        }
    }

    #[cfg(windows)]
    fn ir_string_via_temp_file(&self) -> LlvmResult<String> {
        let unique = format!(
            "nia_llvm_ir_{}_{}.ll",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let path = std::env::temp_dir().join(unique);
        let path_cstr = to_c_string(&path.to_string_lossy())?;
        let mut message = ptr::null_mut();

        let failed =
            unsafe { LLVMPrintModuleToFile(self.raw, path_cstr.as_ptr(), &mut message) } != 0;
        if failed {
            let text = if message.is_null() {
                "Unknown LLVM error".to_string()
            } else {
                let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
                unsafe { LLVMDisposeMessage(message) };
                text
            };
            // LLVM may create or truncate the output before reporting a
            // failure. Do not leave that partial diagnostic artifact behind.
            let _ = std::fs::remove_file(&path);
            return Err(LlvmError::error(text));
        }
        // The message is owned by the caller whenever LLVM supplies one,
        // including an unexpected message on a successful print.
        if !message.is_null() {
            unsafe { LLVMDisposeMessage(message) };
        }

        let read_result = std::fs::read_to_string(&path).map_err(|err| {
            LlvmError::error(format!(
                "Failed to read printed LLVM IR from `{}`: {}",
                path.display(),
                err
            ))
        });
        let _ = std::fs::remove_file(path);
        read_result
    }

    #[cfg(not(windows))]
    fn ir_string_via_llvm_string(&self) -> LlvmResult<String> {
        let text = unsafe { LLVMPrintModuleToString(self.raw) };
        if text.is_null() {
            return Err(LlvmError::error("LLVM returned a null IR buffer"));
        }

        let rendered = unsafe { CStr::from_ptr(text).to_string_lossy().into_owned() };
        unsafe { LLVMDisposeMessage(text) };
        Ok(rendered)
    }

    /// Gets or inserts an intrinsic declaration for the supplied overloads.
    ///
    /// Returns `None` when `name` is not a known LLVM intrinsic or LLVM cannot
    /// produce its declaration.
    pub fn get_intrinsic_declaration(
        &self,
        name: &str,
        types: &[BasicTypeEnum<'ctx>],
    ) -> Option<FunctionValue<'ctx>> {
        let name = name.as_bytes();
        let intrinsic_id =
            unsafe { llvm_sys::core::LLVMLookupIntrinsicID(name.as_ptr() as *const _, name.len()) };
        if intrinsic_id == 0 {
            return None;
        }
        let mut overloads = types.iter().map(|ty| ty.as_type_ref()).collect::<Vec<_>>();
        let value = unsafe {
            LLVMGetIntrinsicDeclaration(
                self.raw,
                intrinsic_id,
                overloads.as_mut_ptr(),
                overloads.len(),
            )
        };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }
}

impl<'ctx> FunctionValue<'ctx> {
    /// Returns the next function in the owning module's iteration order.
    pub fn get_next_function(self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetNextFunction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }
}

impl<'ctx> Drop for Module<'ctx> {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { LLVMDisposeModule(self.raw) };
        }
    }
}

/// Converts a module-level LLVM allocation result into a recoverable error.
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
    fn rejects_null_module_allocations_before_wrapper_construction() {
        let error = require_handle::<llvm_sys::LLVMValue>(std::ptr::null_mut(), "function")
            .expect_err("null function handle");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null function handle".to_string())
        );
    }

    #[test]
    fn rejects_global_initializer_type_mismatch_before_llvm_call() {
        let context = Context::create();
        let module = context.create_module("global-init-type").unwrap();
        let global = module
            .add_global(context.i32_type().into(), None, "value")
            .unwrap();
        let initializer = context.i64_type().const_zero();

        let error = global
            .set_initializer(&initializer)
            .expect_err("global initializer type mismatch");
        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("initializer type does not match")
        ));
    }

    #[test]
    fn preserves_global_address_space_in_pointer_type() {
        let context = Context::create();
        let module = context.create_module("global-address-space").unwrap();
        let global = module
            .add_global(context.i32_type().into(), Some(AddressSpace(3)), "value")
            .unwrap();

        assert_eq!(
            global.as_pointer_value().get_type().address_space(),
            AddressSpace(3)
        );
    }
}
