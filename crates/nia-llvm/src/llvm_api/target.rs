// SPDX-License-Identifier: GPL-3.0-or-later
//! Target machine wrapper for object emission.

use llvm_sys::core::{
    LLVMDisposeMemoryBuffer, LLVMDisposeMessage, LLVMGetBufferSize, LLVMGetBufferStart,
};
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMGetErrorMessage};
use llvm_sys::target::{
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets, LLVMABIAlignmentOfType,
    LLVMABISizeOfType, LLVMDisposeTargetData, LLVMOffsetOfElement, LLVMTargetDataRef,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout,
    LLVMCreateTargetMachine, LLVMDisposeTargetMachine, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef, LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use std::ffi::CStr;
use std::ptr;
use std::slice;
use std::sync::OnceLock;

use super::{
    AsTypeRef, BasicType, LlvmError, LlvmResult, Module, OptimizationLevel, StructType, to_c_string,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A module-local LLVM optimization that is safe before separate object emission.
pub enum ModuleOptimization {
    /// Promote non-address-escaping stack slots to SSA values.
    Mem2Reg,
}

impl ModuleOptimization {
    fn pipeline(self) -> &'static str {
        match self {
            Self::Mem2Reg => "mem2reg",
        }
    }
}

#[derive(Debug)]
/// Owned LLVM target machine used to configure modules and emit objects.
pub struct TargetMachine {
    pub(super) raw: LLVMTargetMachineRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Stable inputs required to recreate a target machine for cached codegen.
pub struct TargetMachineIdentity {
    /// LLVM target triple, such as `x86_64-unknown-linux-gnu`.
    pub triple: String,
    /// CPU model passed to LLVM's target machine constructor.
    pub cpu: String,
    /// LLVM feature string, including enabled and disabled target features.
    pub features: String,
}

impl TargetMachine {
    /// Names the portable machine for `triple`: a baseline CPU and no extra
    /// features, so an artifact runs on every machine of its target rather
    /// than only on the build host.
    pub fn baseline_identity(triple: &str, cpu: &str) -> TargetMachineIdentity {
        TargetMachineIdentity {
            triple: triple.to_string(),
            cpu: cpu.to_string(),
            features: String::new(),
        }
    }

    /// Recreates a target machine from an exact previously captured identity.
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

    /// Creates a target machine from explicit LLVM target inputs.
    pub fn for_triple(
        triple: &str,
        cpu: &str,
        features: &str,
        opt_level: OptimizationLevel,
    ) -> LlvmResult<Self> {
        initialize_targets();

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
        let target_data = self.target_data()?;
        unsafe { module.set_data_layout_from_target(target_data.raw) };
        let triple = unsafe { llvm_sys::target_machine::LLVMGetTargetMachineTriple(self.raw) };
        let triple = llvm_owned_string(triple)?;
        module.set_triple(&triple)?;
        Ok(())
    }

    /// Returns the triple configured on this target machine.
    pub fn target_triple(&self) -> LlvmResult<String> {
        let triple = unsafe { llvm_sys::target_machine::LLVMGetTargetMachineTriple(self.raw) };
        llvm_owned_string(triple)
    }

    /// Runs a typed, module-local optimization before object emission.
    ///
    /// Nia emits separate LLVM modules for separate codegen units. The
    /// pipeline therefore deliberately contains only transformations whose
    /// correctness and benefit do not depend on seeing other modules.
    pub fn run_module_optimization<'ctx>(
        &self,
        module: &Module<'ctx>,
        optimization: ModuleOptimization,
    ) -> LlvmResult<()> {
        let pipeline = to_c_string(optimization.pipeline())?;
        let options = unsafe { LLVMCreatePassBuilderOptions() };
        if options.is_null() {
            return Err(LlvmError::error(
                "LLVM returned a null pass-builder options handle",
            ));
        }
        let error =
            unsafe { LLVMRunPasses(module.as_mut_ptr(), pipeline.as_ptr(), self.raw, options) };
        unsafe { LLVMDisposePassBuilderOptions(options) };
        if error.is_null() {
            return Ok(());
        }
        let message = unsafe { LLVMGetErrorMessage(error) };
        if message.is_null() {
            unsafe { llvm_sys::error::LLVMConsumeError(error) };
            return Err(LlvmError::error("LLVM module optimization failed"));
        }
        let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
        unsafe { LLVMDisposeErrorMessage(message) };
        Err(LlvmError::error(text))
    }

    /// Emits `module` as an in-memory native object file.
    ///
    /// The module must already be configured for this target machine. The
    /// returned bytes are copied out of LLVM-owned storage before it is freed.
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
        // Code generation reports some errors, such as invalid inline
        // assembly, as context diagnostics while still emitting an object.
        let context = unsafe { llvm_sys::core::LLVMGetModuleContext(module.as_mut_ptr()) };
        if let Some(sink) = super::context::DiagnosticSink::of(context) {
            let errors = sink.take_errors();
            if !errors.is_empty() {
                return Err(LlvmError::error(errors.join("\n")));
            }
        }
        Ok(bytes)
    }
}

impl Drop for TargetMachine {
    fn drop(&mut self) {
        unsafe { LLVMDisposeTargetMachine(self.raw) };
    }
}

/// The data layout a target machine gives the modules it configures.
///
/// It answers how LLVM itself places a type, so callers that own a layout
/// contract can check that the LLVM types they build agree with it.
pub struct TargetData {
    raw: LLVMTargetDataRef,
}

impl TargetMachine {
    /// Returns the data layout of this target machine.
    pub fn target_data(&self) -> LlvmResult<TargetData> {
        let raw = unsafe { LLVMCreateTargetDataLayout(self.raw) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null target data layout"));
        }
        Ok(TargetData { raw })
    }
}

impl TargetData {
    /// The allocation size of `ty` in bytes, including tail padding.
    pub fn abi_size<'ctx>(&self, ty: impl BasicType<'ctx>) -> u64 {
        unsafe { LLVMABISizeOfType(self.raw, ty.as_type_ref()) }
    }

    /// The ABI alignment of `ty` in bytes.
    pub fn abi_align<'ctx>(&self, ty: impl BasicType<'ctx>) -> u64 {
        u64::from(unsafe { LLVMABIAlignmentOfType(self.raw, ty.as_type_ref()) })
    }

    /// The byte offset of element `index` of the sized struct `ty`.
    pub fn element_offset<'ctx>(&self, ty: StructType<'ctx>, index: u32) -> u64 {
        unsafe { LLVMOffsetOfElement(self.raw, ty.as_type_ref(), index) }
    }
}

impl Drop for TargetData {
    fn drop(&mut self) {
        unsafe { LLVMDisposeTargetData(self.raw) };
    }
}

/// Registers every target the linked LLVM was built with. The static LLVM
/// build selects that set; triple lookup reports a target outside it.
fn initialize_targets() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| unsafe {
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmPrinters();
        LLVM_InitializeAllAsmParsers();
    });
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

#[cfg(test)]
/// The machine running the tests, at a portable baseline. Production callers
/// always name the artifact target's triple instead.
pub(crate) fn test_host_identity() -> TargetMachineIdentity {
    let triple =
        llvm_owned_string(unsafe { llvm_sys::target_machine::LLVMGetDefaultTargetTriple() })
            .expect("default triple");
    TargetMachine::baseline_identity(&triple, "generic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_target_triple_before_machine_construction() {
        let error = TargetMachine::for_triple(
            "nia-unknown-does-not-exist",
            "generic",
            "",
            OptimizationLevel::None,
        )
        .expect_err("unknown target triple must be rejected");

        assert!(matches!(error, LlvmError::Error(message) if
            message.contains("target") || message.contains("triple")));
    }

    #[test]
    fn rejects_interior_nul_in_every_target_machine_string() {
        for (triple, cpu, features) in [
            ("x86_64-unknown-linux-gnu\0invalid", "generic", ""),
            ("x86_64-unknown-linux-gnu", "generic\0invalid", ""),
            ("x86_64-unknown-linux-gnu", "generic", "+sse2\0invalid"),
        ] {
            let error = TargetMachine::for_triple(triple, cpu, features, OptimizationLevel::None)
                .expect_err("interior NUL must be rejected before calling LLVM");
            assert!(
                matches!(error, LlvmError::Ice(ref ice) if ice.message.contains("interior NUL")),
                "unexpected target-machine error: {error:?}"
            );
        }
    }

    #[test]
    fn configures_module_and_emits_native_object() {
        let context = super::super::Context::create().expect("create LLVM context");
        let module = context
            .create_module("native-object-boundary")
            .expect("create LLVM module");
        let target = TargetMachine::for_identity(&test_host_identity(), OptimizationLevel::None)
            .expect("create native target machine");

        target
            .configure_module(&module)
            .expect("configure module target data and triple");
        module.verify().expect("empty module should verify");
        target
            .run_module_optimization(&module, ModuleOptimization::Mem2Reg)
            .expect("run module optimization");
        let object = target.emit_object(&module).expect("emit native object");
        assert!(
            !object.is_empty(),
            "native object buffer must contain bytes"
        );
    }

    // LLVM reports invalid assembly as a context diagnostic and still returns
    // an object; emission must fail instead of publishing it.
    #[test]
    fn assembler_errors_fail_object_emission() {
        let context = super::super::Context::create().expect("create LLVM context");
        let module = context
            .create_module("invalid-assembly")
            .expect("create LLVM module");
        let target = TargetMachine::for_identity(&test_host_identity(), OptimizationLevel::None)
            .expect("create native target machine");
        target
            .configure_module(&module)
            .expect("configure module target data and triple");
        let assembly = "nia_not_an_instruction";
        unsafe {
            llvm_sys::core::LLVMSetModuleInlineAsm2(
                module.as_mut_ptr(),
                assembly.as_ptr().cast(),
                assembly.len(),
            );
        }
        let error = target
            .emit_object(&module)
            .expect_err("invalid assembly must fail emission");
        assert!(
            matches!(&error, LlvmError::Error(message) if message.contains("nia_not_an_instruction")),
            "{error:?}"
        );
        // The error is reported once; a later emission starts clean.
        unsafe {
            llvm_sys::core::LLVMSetModuleInlineAsm2(module.as_mut_ptr(), "".as_ptr().cast(), 0);
        }
        target
            .emit_object(&module)
            .expect("valid module after an earlier error");
    }
}
