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

impl TargetMachine {
    pub fn native() -> LlvmResult<Self> {
        Self::native_with_opt_level(OptimizationLevel::Default)
    }

    pub fn native_with_opt_level(opt_level: OptimizationLevel) -> LlvmResult<Self> {
        initialize_native_target()?;

        let triple = llvm_owned_string(unsafe { LLVMGetDefaultTargetTriple() })?;
        let cpu = llvm_owned_string(unsafe { LLVMGetHostCPUName() })?;
        let features = llvm_owned_string(unsafe { LLVMGetHostCPUFeatures() })?;
        Self::for_triple(&triple, &cpu, &features, opt_level)
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
            return Err(LlvmError::error(format!(
                "LLVM returned no target for triple `{triple}`"
            )));
        }

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

    pub fn configure_module<'ctx>(&self, module: &Module<'ctx>) -> LlvmResult<()> {
        let target_data = unsafe { LLVMCreateTargetDataLayout(self.raw) };
        if !target_data.is_null() {
            unsafe {
                module.set_data_layout_from_target(target_data);
                LLVMDisposeTargetData(target_data);
            }
        }
        let triple = unsafe { llvm_sys::target_machine::LLVMGetTargetMachineTriple(self.raw) };
        if let Ok(triple) = llvm_owned_string(triple) {
            module.set_triple(&triple)?;
        }
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
            return Err(LlvmError::error(take_llvm_message(
                message,
                "LLVM failed to emit object file",
            )));
        }
        if buffer.is_null() {
            return Err(LlvmError::error("LLVM returned a null object buffer"));
        }

        let bytes = unsafe {
            let start = LLVMGetBufferStart(buffer);
            let len = LLVMGetBufferSize(buffer);
            slice::from_raw_parts(start as *const u8, len).to_vec()
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
