// SPDX-License-Identifier: GPL-3.0-or-later
mod backend_validate;
mod compiler_builtins;
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;

use backend_validate::validate_backend_program;
use module_codegen::ModuleCodegen;
pub use nia_backend_ir::CodegenUnitId;
use nia_backend_ir::{BackendProgram, CodegenPartitionPlan};
use nia_llvm::{Context, OptimizationLevel as LlvmOptimizationLevel, target::TargetMachine};
use nia_opt::NiaOptimizationLevel;
use nia_ty::TypeStore;
pub use output::{
    LlvmCodegenOptions, LlvmCodegenOutput, LlvmModuleOutput, LlvmObjectModuleOutput,
    LlvmObjectOutput,
};
use program_index::ProgramIndex;

pub fn emit_llvm_ir(
    program: &BackendProgram,
    partitions: &CodegenPartitionPlan,
    type_store: &TypeStore,
) -> LlvmCodegenOutput {
    emit_llvm_ir_with_options(
        program,
        partitions,
        type_store,
        LlvmCodegenOptions::default(),
    )
}

pub fn emit_llvm_ir_with_options(
    program: &BackendProgram,
    partitions: &CodegenPartitionPlan,
    type_store: &TypeStore,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    catch_llvm_codegen_ice(|| {
        emit_llvm_ir_with_options_inner(program, partitions, type_store, options)
    })
}

fn emit_llvm_ir_with_options_inner(
    program: &BackendProgram,
    partitions: &CodegenPartitionPlan,
    type_store: &TypeStore,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    partitions.validate_program(program);
    let timings = options.timings;
    let index = time_codegen_stage(timings, "llvm_codegen.program_index", || {
        ProgramIndex::new(program, type_store)
    });
    let validation_diagnostics = time_codegen_stage(timings, "llvm_codegen.validate", || {
        validate_backend_program(program, &index)
    });
    if !validation_diagnostics.is_empty() {
        return LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    for partition in partitions.partitions() {
        match emit_llvm_ir_partition(program, partition, &index, options) {
            Ok(output) => outputs.push(output),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", outputs.len() as u64);
    }
    LlvmCodegenOutput {
        modules: outputs,
        diagnostics,
    }
}

pub fn emit_native_objects(
    program: &BackendProgram,
    partitions: &CodegenPartitionPlan,
    type_store: &TypeStore,
    options: LlvmCodegenOptions,
) -> LlvmObjectOutput {
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    catch_llvm_object_ice(|| emit_native_objects_inner(program, partitions, type_store, options))
}

fn record_memory_permit(timings: nia_timing::TimingMode, waited: bool) {
    if !timings.enabled() {
        return;
    }
    nia_timing::emit_counter("llvm.memory_permits", 1);
    if waited {
        nia_timing::emit_counter("llvm.memory_waits", 1);
    }
}

fn emit_native_objects_inner(
    program: &BackendProgram,
    partitions: &CodegenPartitionPlan,
    type_store: &TypeStore,
    options: LlvmCodegenOptions,
) -> LlvmObjectOutput {
    partitions.validate_program(program);
    let timings = options.timings;
    let target = match time_codegen_stage(timings, "llvm_codegen.native_target", || {
        TargetMachine::native_with_opt_level(llvm_optimization_level(options.optimization.level))
    }) {
        Ok(target) => target,
        Err(error) => {
            return LlvmObjectOutput {
                modules: Vec::new(),
                diagnostics: vec![error.diagnostic()],
            };
        }
    };
    let index = time_codegen_stage(timings, "llvm_codegen.program_index", || {
        ProgramIndex::new(program, type_store)
    });
    let validation_diagnostics = time_codegen_stage(timings, "llvm_codegen.validate", || {
        validate_backend_program(program, &index)
    });
    if !validation_diagnostics.is_empty() {
        return LlvmObjectOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    let builtin_symbols = compiler_builtins::required_symbols(program, &index);
    for partition in partitions.partitions() {
        match emit_native_object_partition(program, partition, &index, &target, options) {
            Ok(output) => outputs.push(output),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if builtin_symbols.any() {
        match compiler_builtins::emit_object(&target, builtin_symbols) {
            Ok(bytes) => outputs.push(LlvmObjectModuleOutput {
                unit: CodegenUnitId::CompilerBuiltins,
                name: "nia.compiler_builtins".to_string(),
                bytes,
            }),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", outputs.len() as u64);
        nia_timing::emit_counter("llvm.object_reuse_hits", 0);
    }
    LlvmObjectOutput {
        modules: outputs,
        diagnostics,
    }
}

fn emit_llvm_ir_partition(
    program: &BackendProgram,
    partition: &nia_backend_ir::CodegenPartition,
    index: &ProgramIndex<'_>,
    options: LlvmCodegenOptions,
) -> Result<LlvmModuleOutput, nia_diagnostic::Diagnostic> {
    let module = program.module_for_partition(partition);
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, index, options)
        })?;
    let ir = codegen.emit_ir()?;
    Ok(LlvmModuleOutput {
        unit: partition.id,
        name: module.name.clone(),
        ir,
    })
}

fn emit_native_object_partition(
    program: &BackendProgram,
    partition: &nia_backend_ir::CodegenPartition,
    index: &ProgramIndex<'_>,
    target: &TargetMachine,
    options: LlvmCodegenOptions,
) -> Result<LlvmObjectModuleOutput, nia_diagnostic::Diagnostic> {
    let module = program.module_for_partition(partition);
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, index, options)
        })?;
    target
        .configure_module(&codegen.module)
        .map_err(|error| error.diagnostic())?;
    let bytes = codegen.emit_object(target)?;
    Ok(LlvmObjectModuleOutput {
        unit: partition.id,
        name: module.name.clone(),
        bytes,
    })
}

pub(crate) fn time_codegen_stage<T>(
    timings: nia_timing::TimingMode,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    nia_timing::time_query(timings, name, f)
}

pub(crate) fn time_codegen_module_stage<T>(
    timings: nia_timing::TimingMode,
    stage: &'static str,
    module_name: &str,
    f: impl FnOnce() -> T,
) -> T {
    if !timings.detail() {
        return f();
    }
    nia_timing::time_query(timings, &format!("llvm_codegen.{stage}[{module_name}]"), f)
}

fn catch_llvm_codegen_ice(f: impl FnOnce() -> LlvmCodegenOutput) -> LlvmCodegenOutput {
    match nia_ice::catch_ice(f) {
        Ok(output) => output,
        Err(ice) => LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: vec![ice.diagnostic()],
        },
    }
}

fn catch_llvm_object_ice(f: impl FnOnce() -> LlvmObjectOutput) -> LlvmObjectOutput {
    match nia_ice::catch_ice(f) {
        Ok(output) => output,
        Err(ice) => LlvmObjectOutput {
            modules: Vec::new(),
            diagnostics: vec![ice.diagnostic()],
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlvmCodegenOptimizationLevel {
    None,
    Less,
    Default,
    Aggressive,
}

impl LlvmCodegenOptimizationLevel {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Less => "less",
            Self::Default => "default",
            Self::Aggressive => "aggressive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlvmCodegenSizePolicy {
    Default,
    Small,
    Tiny,
}

impl LlvmCodegenSizePolicy {
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Small => "small",
            Self::Tiny => "tiny",
        }
    }
}

pub fn llvm_codegen_optimization_level(
    level: NiaOptimizationLevel,
) -> LlvmCodegenOptimizationLevel {
    match level {
        NiaOptimizationLevel::O0 => LlvmCodegenOptimizationLevel::None,
        NiaOptimizationLevel::O1 => LlvmCodegenOptimizationLevel::Less,
        NiaOptimizationLevel::O2 | NiaOptimizationLevel::Os => {
            LlvmCodegenOptimizationLevel::Default
        }
        NiaOptimizationLevel::O3 => LlvmCodegenOptimizationLevel::Aggressive,
        NiaOptimizationLevel::Oz => LlvmCodegenOptimizationLevel::Less,
    }
}

pub fn llvm_codegen_size_policy(level: NiaOptimizationLevel) -> LlvmCodegenSizePolicy {
    match level {
        NiaOptimizationLevel::O0
        | NiaOptimizationLevel::O1
        | NiaOptimizationLevel::O2
        | NiaOptimizationLevel::O3 => LlvmCodegenSizePolicy::Default,
        NiaOptimizationLevel::Os => LlvmCodegenSizePolicy::Small,
        NiaOptimizationLevel::Oz => LlvmCodegenSizePolicy::Tiny,
    }
}

fn llvm_optimization_level(level: NiaOptimizationLevel) -> LlvmOptimizationLevel {
    match llvm_codegen_optimization_level(level) {
        LlvmCodegenOptimizationLevel::None => LlvmOptimizationLevel::None,
        LlvmCodegenOptimizationLevel::Less => LlvmOptimizationLevel::Less,
        LlvmCodegenOptimizationLevel::Default => LlvmOptimizationLevel::Default,
        LlvmCodegenOptimizationLevel::Aggressive => LlvmOptimizationLevel::Aggressive,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod optimization_tests {
    use super::*;

    #[test]
    fn maps_nia_optimization_levels_to_llvm_codegen_levels() {
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::O0),
            LlvmCodegenOptimizationLevel::None
        );
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::O1),
            LlvmCodegenOptimizationLevel::Less
        );
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::O2),
            LlvmCodegenOptimizationLevel::Default
        );
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::O3),
            LlvmCodegenOptimizationLevel::Aggressive
        );
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::Os),
            LlvmCodegenOptimizationLevel::Default
        );
        assert_eq!(
            llvm_codegen_optimization_level(NiaOptimizationLevel::Oz),
            LlvmCodegenOptimizationLevel::Less
        );
    }

    #[test]
    fn llvm_codegen_optimization_level_names_are_stable_for_reports() {
        assert_eq!(LlvmCodegenOptimizationLevel::None.name(), "none");
        assert_eq!(LlvmCodegenOptimizationLevel::Less.name(), "less");
        assert_eq!(LlvmCodegenOptimizationLevel::Default.name(), "default");
        assert_eq!(
            LlvmCodegenOptimizationLevel::Aggressive.name(),
            "aggressive"
        );
    }

    #[test]
    fn maps_nia_size_levels_to_llvm_codegen_size_policy() {
        for level in [
            NiaOptimizationLevel::O0,
            NiaOptimizationLevel::O1,
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
        ] {
            assert_eq!(
                llvm_codegen_size_policy(level),
                LlvmCodegenSizePolicy::Default,
                "{level:?}"
            );
        }
        assert_eq!(
            llvm_codegen_size_policy(NiaOptimizationLevel::Os),
            LlvmCodegenSizePolicy::Small
        );
        assert_eq!(
            llvm_codegen_size_policy(NiaOptimizationLevel::Oz),
            LlvmCodegenSizePolicy::Tiny
        );
    }

    #[test]
    fn llvm_codegen_size_policy_names_are_stable_for_reports() {
        assert_eq!(LlvmCodegenSizePolicy::Default.name(), "default");
        assert_eq!(LlvmCodegenSizePolicy::Small.name(), "small");
        assert_eq!(LlvmCodegenSizePolicy::Tiny.name(), "tiny");
    }
}
