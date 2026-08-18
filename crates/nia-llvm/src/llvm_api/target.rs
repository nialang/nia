// SPDX-License-Identifier: GPL-3.0-or-later
//! Native target machine wrapper for object emission.

use llvm_sys::core::{
    LLVMDisposeMemoryBuffer, LLVMDisposeMessage, LLVMGetBufferSize, LLVMGetBufferStart,
};
use llvm_sys::target::{
    LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
    LLVMDisposeTargetData,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout,
    LLVMCreateTargetMachine, LLVMDisposeTargetMachine, LLVMGetDefaultTargetTriple,
    LLVMGetHostCPUFeatures, LLVMGetHostCPUName, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef, LLVMTargetRef,
};
use std::ffi::CStr;
use std::ptr;
use std::slice;
use std::sync::OnceLock;

use super::{LlvmError, LlvmResult, Module, OptimizationLevel, to_c_string};

#[derive(Debug)]
pub struct TargetMachine {
    raw: LLVMTargetMachineRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetMachineIdentity {
    pub triple: String,
    pub cpu: String,
    pub features: String,
}

impl TargetMachine {
    pub fn native() -> LlvmResult<Self> {
        Self::native_with_opt_level(OptimizationLevel::Default)
    }

    pub fn native_with_opt_level(opt_level: OptimizationLevel) -> LlvmResult<Self> {
        let identity = Self::native_identity()?;
        Self::for_identity(&identity, opt_level)
    }

    pub fn native_identity() -> LlvmResult<TargetMachineIdentity> {
        initialize_native_target()?;
        Ok(TargetMachineIdentity {
            triple: llvm_owned_string(unsafe { LLVMGetDefaultTargetTriple() })?,
            cpu: llvm_owned_string(unsafe { LLVMGetHostCPUName() })?,
            features: llvm_owned_string(unsafe { LLVMGetHostCPUFeatures() })?,
        })
    }

    pub fn for_identity(
        identity: &TargetMachineIdentity,
        opt_level: OptimizationLevel,
    ) -> LlvmResult<Self> {
        Self::for_triple(
            &identity.triple,
            &identity.cpu,
            &identity.features,
            opt_level,
        )
    }

    pub fn for_triple(
        triple: &str,
        cpu: &str,
        features: &str,
        opt_level: OptimizationLevel,
    ) -> LlvmResult<Self> {
        initialize_native_target()?;

        let triple_c = to_c_string(triple)?;
        let mut target: LLVMTargetRef = ptr::null_mut();
        let mut message = ptr::null_mut();
        let failed =
            unsafe { LLVMGetTargetFromTriple(triple_c.as_ptr(), &mut target, &mut message) } != 0;
        if failed {
            return Err(LlvmError::error(take_llvm_message(
                message,
                "LLVM failed to find target for triple",
            )));
        }
        if target.is_null() {
            dispose_llvm_message(message);
            return Err(LlvmError::error(format!(
                "LLVM returned no target for triple `{triple}`"
            )));
        }
        // The API documents the message as an error result, but retain full
        // ownership discipline if a future LLVM version supplies one together
        // with a successful target lookup.
        dispose_llvm_message(message);

        let cpu_c = to_c_string(cpu)?;
        let features_c = to_c_string(features)?;
        let machine = unsafe {
            LLVMCreateTargetMachine(
                target,
                triple_c.as_ptr(),
                cpu_c.as_ptr(),
                features_c.as_ptr(),
                codegen_opt_level(opt_level),
                LLVMRelocMode::LLVMRelocPIC,
                LLVMCodeModel::LLVMCodeModelDefault,
            )
        };
        if machine.is_null() {
            return Err(LlvmError::error(format!(
                "LLVM failed to create target machine for triple `{triple}`"
            )));
        }
        Ok(Self { raw: machine })
    }

    /// Attach this machine's target layout and triple to `module`.
    ///
    /// A target data layout is part of the module/codegen contract, not an
    /// optional optimization hint. LLVM can return a null layout handle for a
    /// malformed or unusable target machine; surface that failure instead of
    /// allowing later size/alignment queries to observe a stale layout.
    pub fn configure_module<'ctx>(&self, module: &Module<'ctx>) -> LlvmResult<()> {
        let target_data = unsafe { LLVMCreateTargetDataLayout(self.raw) };
        if target_data.is_null() {
            return Err(LlvmError::error("LLVM returned a null target data layout"));
        }
        unsafe {
            module.set_data_layout_from_target(target_data);
            LLVMDisposeTargetData(target_data);
        }
        let triple = unsafe { llvm_sys::target_machine::LLVMGetTargetMachineTriple(self.raw) };
        let triple = llvm_owned_string(triple)?;
        module.set_triple(&triple)?;
        Ok(())
    }

    pub fn emit_object<'ctx>(&self, module: &Module<'ctx>) -> LlvmResult<Vec<u8>> {
        let mut message = ptr::null_mut();
        let mut buffer = ptr::null_mut();
        let failed = unsafe {
            LLVMTargetMachineEmitToMemoryBuffer(
                self.raw,
                module.as_mut_ptr(),
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut message,
                &mut buffer,
            )
        } != 0;
        if failed {
            if !buffer.is_null() {
                unsafe { LLVMDisposeMemoryBuffer(buffer) };
            }
            return Err(LlvmError::error(take_llvm_message(
                message,
                "LLVM failed to emit object file",
            )));
        }
        // As above, do not assume successful calls always leave the optional
        // owned message pointer null.
        dispose_llvm_message(message);
        if buffer.is_null() {
            return Err(LlvmError::error("LLVM returned a null object buffer"));
        }

        let bytes = unsafe {
            let start = LLVMGetBufferStart(buffer);
            let len = LLVMGetBufferSize(buffer);
            // `from_raw_parts` still requires a non-null, aligned pointer for
            // a zero-length slice. LLVM normally emits a non-empty object, but
            // keep the wrapper correct for empty/mocked buffers as well.
            if len == 0 {
                Vec::new()
            } else if start.is_null() {
                LLVMDisposeMemoryBuffer(buffer);
                return Err(LlvmError::error(
                    "LLVM returned an object buffer with a null start",
                ));
            } else {
                slice::from_raw_parts(start as *const u8, len).to_vec()
            }
        };
        unsafe { LLVMDisposeMemoryBuffer(buffer) };
        Ok(bytes)
    }
}

impl Drop for TargetMachine {
    fn drop(&mut self) {
        unsafe { LLVMDisposeTargetMachine(self.raw) };
    }
}

fn initialize_native_target() -> LlvmResult<()> {
    static RESULT: OnceLock<LlvmResult<()>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            if unsafe { LLVM_InitializeNativeTarget() } != 0 {
                return Err(LlvmError::error("LLVM failed to initialize native target"));
            }
            if unsafe { LLVM_InitializeNativeAsmPrinter() } != 0 {
                return Err(LlvmError::error(
                    "LLVM failed to initialize native asm printer",
                ));
            }
            if unsafe { LLVM_InitializeNativeAsmParser() } != 0 {
                return Err(LlvmError::error(
                    "LLVM failed to initialize native asm parser",
                ));
            }
            Ok(())
        })
        .clone()
}

fn codegen_opt_level(level: OptimizationLevel) -> LLVMCodeGenOptLevel {
    match level {
        OptimizationLevel::None => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
        OptimizationLevel::Less => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
        OptimizationLevel::Default => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
        OptimizationLevel::Aggressive => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
    }
}

fn llvm_owned_string(ptr: *mut std::os::raw::c_char) -> LlvmResult<String> {
    if ptr.is_null() {
        return Err(LlvmError::error("LLVM returned a null string"));
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { LLVMDisposeMessage(ptr) };
    Ok(text)
}

fn take_llvm_message(ptr: *mut std::os::raw::c_char, fallback: &str) -> String {
    if ptr.is_null() {
        return fallback.to_string();
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { LLVMDisposeMessage(ptr) };
    text
}

fn dispose_llvm_message(ptr: *mut std::os::raw::c_char) {
    if !ptr.is_null() {
        unsafe { LLVMDisposeMessage(ptr) };
    }
}
