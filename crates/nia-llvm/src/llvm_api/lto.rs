// SPDX-License-Identifier: GPL-3.0-or-later
//! Safe ownership boundary for LLVM's modern resolution-based ThinLTO API.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::slice;
use std::time::Duration;

use llvm_sys::prelude::LLVMModuleRef;
use llvm_sys::target_machine::LLVMTargetMachineRef;

use super::{
    LlvmError, LlvmResult, Module, OptimizationLevel, TargetMachine, TargetMachineIdentity,
};

#[derive(Debug, Clone, Copy)]
/// One summary-bearing module supplied to the ThinLTO coordinator.
pub struct ThinLtoInput<'a> {
    /// Unique module identifier used by the combined summary index.
    pub name: &'a str,
    /// Bitcode emitted by [`emit_thin_lto_bitcode`].
    pub bitcode: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
/// Complete target and policy inputs for one ThinLTO coordination run.
pub struct ThinLtoConfig<'a> {
    /// Exact LLVM target identity shared by every input module.
    pub target: &'a TargetMachineIdentity,
    /// Pre-link, import, post-link, and code-generation optimization level.
    pub optimization: OptimizationLevel,
    /// Maximum number of in-process ThinLTO backend workers.
    pub parallelism: usize,
    /// Disable assumptions about hosted target-library functions.
    pub freestanding: bool,
    /// Definitions that must remain visible to native linker inputs.
    pub preserved_symbols: &'a [&'a str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Severity attached to a diagnostic emitted by LLVM during ThinLTO.
pub enum ThinLtoDiagnosticSeverity {
    /// LLVM rejected an input or transformation.
    Error,
    /// LLVM reported a recoverable warning.
    Warning,
    /// Optimization remark.
    Remark,
    /// Supplemental diagnostic note.
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Diagnostic retained from the modern LLVM LTO pipeline.
pub struct ThinLtoDiagnostic {
    /// LLVM diagnostic severity.
    pub severity: ThinLtoDiagnosticSeverity,
    /// Rendered diagnostic text.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Native object emitted for one ThinLTO backend task.
pub struct ThinLtoObject {
    /// LLVM task identity, stable for a fixed ordered input set.
    pub task: u32,
    /// Input module identifier associated with this object.
    pub module_name: String,
    /// Native object bytes copied out of the LLVM output stream.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Separately attributable ThinLTO wall and accumulated task time.
pub struct ThinLtoTimings {
    /// Whole-program summary combination and import-list analysis wall time.
    pub thin_link: Duration,
    /// Parallel backend batch wall time.
    pub backend: Duration,
    /// Accumulated per-task promotion time.
    pub promotion: Duration,
    /// Accumulated per-task internalization time.
    pub internalization: Duration,
    /// Accumulated per-task import time.
    pub import: Duration,
    /// Accumulated per-task post-link optimization time.
    pub optimization: Duration,
    /// Accumulated per-task native code-generation time.
    pub codegen: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Native ThinLTO work products and pipeline evidence.
pub struct ThinLtoOutput {
    /// Objects sorted by LLVM task identity.
    pub objects: Vec<ThinLtoObject>,
    /// Non-fatal diagnostics retained in emission order.
    pub diagnostics: Vec<ThinLtoDiagnostic>,
    /// Separately measured global and per-task stages.
    pub timings: ThinLtoTimings,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FfiByteSlice {
    data: *const u8,
    len: usize,
}

impl FfiByteSlice {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

#[repr(C)]
struct FfiThinInput {
    name: FfiByteSlice,
    bitcode: FfiByteSlice,
}

#[repr(C)]
struct FfiThinConfig {
    cpu: FfiByteSlice,
    features: FfiByteSlice,
    preserved_symbols: *const FfiByteSlice,
    preserved_symbol_count: usize,
    optimization: u32,
    parallelism: u32,
    freestanding: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FfiThinTimings {
    thin_link_ns: u64,
    backend_ns: u64,
    promotion_ns: u64,
    internalization_ns: u64,
    import_ns: u64,
    optimization_ns: u64,
    codegen_ns: u64,
}

enum FfiOwnedBuffer {}
enum FfiThinResult {}

unsafe extern "C" {
    fn nia_llvm_emit_thin_lto_bitcode(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        optimization: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_owned_buffer_data(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_error(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_error_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_free(buffer: *mut FfiOwnedBuffer);

    fn nia_llvm_run_thin_lto(
        inputs: *const FfiThinInput,
        input_count: usize,
        config: *const FfiThinConfig,
    ) -> *mut FfiThinResult;
    fn nia_llvm_thin_result_object_count(result: *const FfiThinResult) -> usize;
    fn nia_llvm_thin_result_object_task(result: *const FfiThinResult, index: usize) -> u32;
    fn nia_llvm_thin_result_object_name(result: *const FfiThinResult, index: usize) -> *const u8;
    fn nia_llvm_thin_result_object_name_len(result: *const FfiThinResult, index: usize) -> usize;
    fn nia_llvm_thin_result_object_data(result: *const FfiThinResult, index: usize) -> *const u8;
    fn nia_llvm_thin_result_object_len(result: *const FfiThinResult, index: usize) -> usize;
    fn nia_llvm_thin_result_diagnostic_count(result: *const FfiThinResult) -> usize;
    fn nia_llvm_thin_result_diagnostic_severity(result: *const FfiThinResult, index: usize) -> u32;
    fn nia_llvm_thin_result_diagnostic_message(
        result: *const FfiThinResult,
        index: usize,
    ) -> *const u8;
    fn nia_llvm_thin_result_diagnostic_message_len(
        result: *const FfiThinResult,
        index: usize,
    ) -> usize;
    fn nia_llvm_thin_result_error(result: *const FfiThinResult) -> *const u8;
    fn nia_llvm_thin_result_error_len(result: *const FfiThinResult) -> usize;
    fn nia_llvm_thin_result_timings(result: *const FfiThinResult) -> FfiThinTimings;
    fn nia_llvm_thin_result_free(result: *mut FfiThinResult);
}

struct OwnedBufferHandle(NonNull<FfiOwnedBuffer>);

impl Drop for OwnedBufferHandle {
    fn drop(&mut self) {
        unsafe { nia_llvm_owned_buffer_free(self.0.as_ptr()) };
    }
}

struct ThinResultHandle(NonNull<FfiThinResult>);

impl Drop for ThinResultHandle {
    fn drop(&mut self) {
        unsafe { nia_llvm_thin_result_free(self.0.as_ptr()) };
    }
}

/// Runs target-aware ThinLTO pre-link optimization and emits summary-bearing bitcode.
pub fn emit_thin_lto_bitcode(
    module: &Module<'_>,
    target: &TargetMachine,
    optimization: OptimizationLevel,
) -> LlvmResult<Vec<u8>> {
    let result = unsafe {
        nia_llvm_emit_thin_lto_bitcode(module.raw, target.raw, optimization_tag(optimization))
    };
    let result = OwnedBufferHandle(
        NonNull::new(result)
            .ok_or_else(|| LlvmError::error("LLVM returned no ThinLTO pre-link result"))?,
    );
    let error = unsafe {
        copy_text(
            nia_llvm_owned_buffer_error(result.0.as_ptr()),
            nia_llvm_owned_buffer_error_len(result.0.as_ptr()),
            "ThinLTO pre-link error",
        )?
    };
    if !error.is_empty() {
        return Err(LlvmError::error(error));
    }
    unsafe {
        copy_bytes(
            nia_llvm_owned_buffer_data(result.0.as_ptr()),
            nia_llvm_owned_buffer_len(result.0.as_ptr()),
            "ThinLTO pre-link bitcode",
        )
    }
}

/// Runs a complete modern LLVM ThinLTO link over ordered summary-bearing modules.
pub fn run_thin_lto(
    inputs: &[ThinLtoInput<'_>],
    config: ThinLtoConfig<'_>,
) -> LlvmResult<ThinLtoOutput> {
    let parallelism = u32::try_from(config.parallelism)
        .map_err(|_| LlvmError::error("ThinLTO parallelism exceeds LLVM's supported width"))?;
    if parallelism == 0 {
        return Err(LlvmError::error(
            "ThinLTO requires at least one backend worker",
        ));
    }
    let ffi_inputs = inputs
        .iter()
        .map(|input| FfiThinInput {
            name: FfiByteSlice::new(input.name.as_bytes()),
            bitcode: FfiByteSlice::new(input.bitcode),
        })
        .collect::<Vec<_>>();
    let preserved = config
        .preserved_symbols
        .iter()
        .map(|symbol| FfiByteSlice::new(symbol.as_bytes()))
        .collect::<Vec<_>>();
    let ffi_config = FfiThinConfig {
        cpu: FfiByteSlice::new(config.target.cpu.as_bytes()),
        features: FfiByteSlice::new(config.target.features.as_bytes()),
        preserved_symbols: preserved.as_ptr(),
        preserved_symbol_count: preserved.len(),
        optimization: optimization_tag(config.optimization),
        parallelism,
        freestanding: u8::from(config.freestanding),
    };
    let result =
        unsafe { nia_llvm_run_thin_lto(ffi_inputs.as_ptr(), ffi_inputs.len(), &ffi_config) };
    let result = ThinResultHandle(
        NonNull::new(result).ok_or_else(|| LlvmError::error("LLVM returned no ThinLTO result"))?,
    );
    let error = unsafe {
        copy_text(
            nia_llvm_thin_result_error(result.0.as_ptr()),
            nia_llvm_thin_result_error_len(result.0.as_ptr()),
            "ThinLTO error",
        )?
    };
    if !error.is_empty() {
        return Err(LlvmError::error(error));
    }

    let object_count = unsafe { nia_llvm_thin_result_object_count(result.0.as_ptr()) };
    let mut objects = Vec::with_capacity(object_count);
    for index in 0..object_count {
        let module_name = unsafe {
            copy_text(
                nia_llvm_thin_result_object_name(result.0.as_ptr(), index),
                nia_llvm_thin_result_object_name_len(result.0.as_ptr(), index),
                "ThinLTO object module name",
            )?
        };
        let bytes = unsafe {
            copy_bytes(
                nia_llvm_thin_result_object_data(result.0.as_ptr(), index),
                nia_llvm_thin_result_object_len(result.0.as_ptr(), index),
                "ThinLTO object",
            )?
        };
        objects.push(ThinLtoObject {
            task: unsafe { nia_llvm_thin_result_object_task(result.0.as_ptr(), index) },
            module_name,
            bytes,
        });
    }

    let diagnostic_count = unsafe { nia_llvm_thin_result_diagnostic_count(result.0.as_ptr()) };
    let mut diagnostics = Vec::with_capacity(diagnostic_count);
    for index in 0..diagnostic_count {
        let severity =
            match unsafe { nia_llvm_thin_result_diagnostic_severity(result.0.as_ptr(), index) } {
                0 => ThinLtoDiagnosticSeverity::Error,
                1 => ThinLtoDiagnosticSeverity::Warning,
                2 => ThinLtoDiagnosticSeverity::Remark,
                3 => ThinLtoDiagnosticSeverity::Note,
                other => {
                    return Err(LlvmError::error(format!(
                        "LLVM returned unknown ThinLTO diagnostic severity {other}"
                    )));
                }
            };
        let message = unsafe {
            copy_text(
                nia_llvm_thin_result_diagnostic_message(result.0.as_ptr(), index),
                nia_llvm_thin_result_diagnostic_message_len(result.0.as_ptr(), index),
                "ThinLTO diagnostic",
            )?
        };
        diagnostics.push(ThinLtoDiagnostic { severity, message });
    }

    let timings = unsafe { nia_llvm_thin_result_timings(result.0.as_ptr()) };
    Ok(ThinLtoOutput {
        objects,
        diagnostics,
        timings: ThinLtoTimings {
            thin_link: Duration::from_nanos(timings.thin_link_ns),
            backend: Duration::from_nanos(timings.backend_ns),
            promotion: Duration::from_nanos(timings.promotion_ns),
            internalization: Duration::from_nanos(timings.internalization_ns),
            import: Duration::from_nanos(timings.import_ns),
            optimization: Duration::from_nanos(timings.optimization_ns),
            codegen: Duration::from_nanos(timings.codegen_ns),
        },
    })
}

fn optimization_tag(level: OptimizationLevel) -> u32 {
    match level {
        OptimizationLevel::None => 0,
        OptimizationLevel::Less => 1,
        OptimizationLevel::Default => 2,
        OptimizationLevel::Aggressive => 3,
    }
}

unsafe fn copy_bytes(pointer: *const u8, len: usize, label: &str) -> LlvmResult<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err(LlvmError::error(format!(
            "LLVM returned null {label} bytes with non-zero length"
        )));
    }
    Ok(unsafe { slice::from_raw_parts(pointer, len) }.to_vec())
}

unsafe fn copy_text(pointer: *const u8, len: usize, label: &str) -> LlvmResult<String> {
    let bytes = unsafe { copy_bytes(pointer, len, label)? };
    String::from_utf8(bytes)
        .map_err(|_| LlvmError::error(format!("LLVM returned non-UTF-8 {label} text")))
}

const _: () = {
    assert!(std::mem::size_of::<*const c_void>() == std::mem::size_of::<*const FfiThinResult>());
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, module::Linkage};

    fn summary_bitcode(name: &str) -> (TargetMachineIdentity, Vec<u8>) {
        let context = Context::create().expect("create context");
        let module = context.create_module(name).expect("create module");
        module.set_identifier(name);
        let ty = context
            .i32_type()
            .fn_type(&[], false)
            .expect("create function type");
        let function = module
            .add_function("entry", ty, Some(Linkage::External))
            .expect("add entry function");
        let block = context
            .append_basic_block(function, "entry")
            .expect("append block");
        let builder = context.create_builder().expect("create builder");
        builder.position_at_end(block);
        let value = context.i32_type().const_int(42, false).expect("constant");
        builder.build_return(Some(&value)).expect("return value");
        module.verify().expect("verify source module");

        let identity = TargetMachine::native_identity().expect("native target identity");
        let target = TargetMachine::for_identity(&identity, OptimizationLevel::Default)
            .expect("target machine");
        target.configure_module(&module).expect("configure module");
        let bitcode = emit_thin_lto_bitcode(&module, &target, OptimizationLevel::Default)
            .expect("emit summary-bearing bitcode");
        (identity, bitcode)
    }

    #[test]
    fn modern_thin_lto_emits_native_object_from_summary_bitcode() {
        let (target, bitcode) = summary_bitcode("thin-unit");
        let output = run_thin_lto(
            &[ThinLtoInput {
                name: "thin-unit",
                bitcode: &bitcode,
            }],
            ThinLtoConfig {
                target: &target,
                optimization: OptimizationLevel::Default,
                parallelism: 1,
                freestanding: true,
                preserved_symbols: &["entry"],
            },
        )
        .expect("run modern ThinLTO");

        assert_eq!(output.objects.len(), 1);
        assert!(!output.objects[0].bytes.is_empty());
        assert!(
            output
                .diagnostics
                .iter()
                .all(|diagnostic| { diagnostic.severity != ThinLtoDiagnosticSeverity::Error })
        );
    }

    #[test]
    fn modern_thin_lto_resolves_cross_module_calls() {
        let identity = TargetMachine::native_identity().expect("native target identity");
        let target = TargetMachine::for_identity(&identity, OptimizationLevel::Default)
            .expect("target machine");

        let definition_context = Context::create().expect("create definition context");
        let definition_module = definition_context
            .create_module("definition")
            .expect("create definition module");
        definition_module.set_identifier("definition");
        target
            .configure_module(&definition_module)
            .expect("configure definition module");
        let definition_ty = definition_context
            .i32_type()
            .fn_type(&[], false)
            .expect("create definition type");
        let definition = definition_module
            .add_function("shared_value", definition_ty, Some(Linkage::External))
            .expect("add shared definition");
        let definition_block = definition_context
            .append_basic_block(definition, "entry")
            .expect("append definition block");
        let definition_builder = definition_context
            .create_builder()
            .expect("create definition builder");
        definition_builder.position_at_end(definition_block);
        let value = definition_context
            .i32_type()
            .const_int(42, false)
            .expect("definition value");
        definition_builder
            .build_return(Some(&value))
            .expect("return definition value");
        definition_module
            .verify()
            .expect("verify definition module");
        let definition_bitcode =
            emit_thin_lto_bitcode(&definition_module, &target, OptimizationLevel::Default)
                .expect("emit definition bitcode");

        let caller_context = Context::create().expect("create caller context");
        let caller_module = caller_context
            .create_module("caller")
            .expect("create caller module");
        caller_module.set_identifier("caller");
        target
            .configure_module(&caller_module)
            .expect("configure caller module");
        let caller_ty = caller_context
            .i32_type()
            .fn_type(&[], false)
            .expect("create caller type");
        let shared = caller_module
            .add_function("shared_value", caller_ty, Some(Linkage::External))
            .expect("declare shared function");
        let entry = caller_module
            .add_function("entry", caller_ty, Some(Linkage::External))
            .expect("add entry function");
        let caller_block = caller_context
            .append_basic_block(entry, "entry")
            .expect("append caller block");
        let caller_builder = caller_context
            .create_builder()
            .expect("create caller builder");
        caller_builder.position_at_end(caller_block);
        let call = caller_builder
            .build_call(shared, &[], "shared")
            .expect("call shared function")
            .try_as_basic_value()
            .unwrap_basic()
            .expect("non-void shared result");
        caller_builder
            .build_return(Some(&call))
            .expect("return shared result");
        caller_module.verify().expect("verify caller module");
        let caller_bitcode =
            emit_thin_lto_bitcode(&caller_module, &target, OptimizationLevel::Default)
                .expect("emit caller bitcode");

        let output = run_thin_lto(
            &[
                ThinLtoInput {
                    name: "definition",
                    bitcode: &definition_bitcode,
                },
                ThinLtoInput {
                    name: "caller",
                    bitcode: &caller_bitcode,
                },
            ],
            ThinLtoConfig {
                target: &identity,
                optimization: OptimizationLevel::Default,
                parallelism: 2,
                freestanding: true,
                preserved_symbols: &["entry"],
            },
        )
        .expect("run cross-module ThinLTO");

        assert_eq!(output.objects.len(), 2);
        assert!(output.objects.iter().all(|object| !object.bytes.is_empty()));
        assert!(
            output
                .diagnostics
                .iter()
                .all(|diagnostic| { diagnostic.severity != ThinLtoDiagnosticSeverity::Error })
        );
    }

    #[test]
    fn rejects_bitcode_without_thin_lto_summary() {
        let context = Context::create().expect("create context");
        let module = context.create_module("ordinary").expect("create module");
        let bitcode = module.bitcode().expect("ordinary bitcode");
        let target = TargetMachine::native_identity().expect("native target identity");
        let error = run_thin_lto(
            &[ThinLtoInput {
                name: "ordinary",
                bitcode: &bitcode,
            }],
            ThinLtoConfig {
                target: &target,
                optimization: OptimizationLevel::None,
                parallelism: 1,
                freestanding: true,
                preserved_symbols: &[],
            },
        )
        .expect_err("ordinary bitcode must not enter the ThinLTO coordinator");

        assert!(matches!(error, LlvmError::Error(message) if message.contains("summary")));
    }

    #[test]
    fn rejects_duplicate_module_names_before_index_construction() {
        let (target, bitcode) = summary_bitcode("duplicate");
        let inputs = [
            ThinLtoInput {
                name: "duplicate",
                bitcode: &bitcode,
            },
            ThinLtoInput {
                name: "duplicate",
                bitcode: &bitcode,
            },
        ];
        let error = run_thin_lto(
            &inputs,
            ThinLtoConfig {
                target: &target,
                optimization: OptimizationLevel::None,
                parallelism: 1,
                freestanding: true,
                preserved_symbols: &[],
            },
        )
        .expect_err("duplicate names must be rejected");

        assert!(matches!(error, LlvmError::Error(message) if message.contains("unique")));
    }
}
