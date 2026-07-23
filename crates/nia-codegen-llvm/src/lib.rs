// SPDX-License-Identifier: GPL-3.0-or-later
mod backend_validate;
mod compiler_builtins;
mod fingerprint;
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;
mod work_product;

use std::sync::Arc;

use backend_validate::validate_backend_program;
use module_codegen::ModuleCodegen;
use nia_backend_ir::CodegenPartition;
pub use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey};
use nia_backend_lower::BackendLowering;
use nia_llvm::{Context, OptimizationLevel as LlvmOptimizationLevel, target::TargetMachine};
use nia_opt::NiaOptimizationLevel;
use nia_query::QuerySession;
use nia_ty::TypeStore;
pub use output::{
    LlvmCodegenOptions, LlvmCodegenOutput, LlvmModuleOutput, LlvmObjectModuleOutput,
    LlvmObjectOutput,
};
use program_index::ProgramIndex;
pub use work_product::ObjectWorkProductCache;

pub fn emit_llvm_ir(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
) -> LlvmCodegenOutput {
    emit_llvm_ir_with_options(lowering, type_store, session, LlvmCodegenOptions::default())
}

pub fn emit_llvm_ir_with_options(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    catch_llvm_codegen_ice(|| {
        emit_llvm_ir_with_options_inner(lowering, type_store, session, options)
    })
}

fn emit_llvm_ir_with_options_inner(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    let timings = options.timings;
    lowering
        .codegen_partitions
        .validate_program(&lowering.program);
    let partitions = lowering.codegen_partitions.partitions().to_vec();
    let index = Arc::new(time_codegen_stage(
        timings,
        "llvm_codegen.program_index",
        || ProgramIndex::new(lowering, type_store),
    ));
    let validation_diagnostics = time_codegen_stage(timings, "llvm_codegen.validate", || {
        validate_backend_program(index.program(), &index)
    });
    if !validation_diagnostics.is_empty() {
        return LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let outcomes = session.run_tasks(partitions.into_iter().map(|partition| {
        let index = Arc::clone(&index);
        move || emit_llvm_ir_partition(partition, index, options)
    }));
    let mut outputs = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    for outcome in outcomes {
        match outcome {
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
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
    cache: Option<Arc<dyn ObjectWorkProductCache>>,
) -> LlvmObjectOutput {
    catch_llvm_object_ice(|| {
        emit_native_objects_inner(lowering, type_store, session, options, cache)
    })
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
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
    cache: Option<Arc<dyn ObjectWorkProductCache>>,
) -> LlvmObjectOutput {
    let timings = options.timings;
    lowering
        .codegen_partitions
        .validate_program(&lowering.program);
    let partitions = lowering.codegen_partitions.partitions().to_vec();
    let index = Arc::new(time_codegen_stage(
        timings,
        "llvm_codegen.program_index",
        || ProgramIndex::new(lowering, type_store),
    ));
    let validation_diagnostics = time_codegen_stage(timings, "llvm_codegen.validate", || {
        validate_backend_program(index.program(), &index)
    });
    if !validation_diagnostics.is_empty() {
        return LlvmObjectOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let builtin_symbols = compiler_builtins::required_symbols(index.program(), &index);
    let mut tasks = partitions
        .into_iter()
        .map(NativeCodegenTask::Partition)
        .collect::<Vec<_>>();
    if builtin_symbols.any() {
        tasks.push(NativeCodegenTask::CompilerBuiltins(builtin_symbols));
    }
    let outcomes = session.run_tasks(tasks.into_iter().map(|task| {
        let index = Arc::clone(&index);
        let cache = cache.clone();
        move || match task {
            NativeCodegenTask::Partition(partition) => {
                emit_native_object_partition(partition, index, options, cache.as_deref())
            }
            NativeCodegenTask::CompilerBuiltins(symbols) => {
                emit_compiler_builtins_object(symbols, options, cache.as_deref())
            }
        }
    }));
    let mut outputs = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    let mut reuse_hits = 0;
    for outcome in outcomes {
        match outcome {
            Ok((output, reused)) => {
                reuse_hits += usize::from(reused);
                outputs.push(output);
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", outputs.len() as u64);
        nia_timing::emit_counter("llvm.object_reuse_hits", reuse_hits as u64);
        nia_timing::emit_counter(
            "llvm.object_reuse_misses",
            outputs.len().saturating_sub(reuse_hits) as u64,
        );
    }
    LlvmObjectOutput {
        modules: outputs,
        diagnostics,
    }
}

enum NativeCodegenTask {
    Partition(CodegenPartition),
    CompilerBuiltins(compiler_builtins::CompilerBuiltinSymbols),
}

fn emit_llvm_ir_partition(
    partition: CodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
) -> Result<LlvmModuleOutput, nia_diagnostic::Diagnostic> {
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let module = index.program().module_for_partition(&partition);
    let fingerprint = fingerprint::source_unit_fingerprint(
        &partition,
        &index,
        options,
        fingerprint::ArtifactTarget::LlvmIr,
    );
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, &index, options)
        })?;
    let ir = codegen.emit_ir()?;
    Ok(LlvmModuleOutput {
        unit: partition.id,
        key: partition.key,
        fingerprint,
        name: module.name.clone(),
        ir,
    })
}

fn emit_native_object_partition(
    partition: CodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(LlvmObjectModuleOutput, bool), nia_diagnostic::Diagnostic> {
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| error.diagnostic())?;
    let fingerprint = fingerprint::source_unit_fingerprint(
        &partition,
        &index,
        options,
        fingerprint::ArtifactTarget::NativeObject(&target_identity),
    );
    let module = index.program().module_for_partition(&partition);
    if let Some(bytes) = load_object_work_product(cache, &partition.key, fingerprint) {
        return Ok((
            LlvmObjectModuleOutput {
                unit: partition.id,
                key: partition.key,
                fingerprint,
                name: module.name.clone(),
                bytes,
            },
            true,
        ));
    }
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            &target_identity,
            llvm_optimization_level(options.optimization.level),
        )
    })
    .map_err(|error| error.diagnostic())?;
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, &index, options)
        })?;
    target
        .configure_module(&codegen.module)
        .map_err(|error| error.diagnostic())?;
    let bytes = codegen.emit_object(&target)?;
    publish_object_work_product(cache, &partition.key, fingerprint, &bytes);
    Ok((
        LlvmObjectModuleOutput {
            unit: partition.id,
            key: partition.key,
            fingerprint,
            name: module.name.clone(),
            bytes,
        },
        false,
    ))
}

fn emit_compiler_builtins_object(
    symbols: compiler_builtins::CompilerBuiltinSymbols,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(LlvmObjectModuleOutput, bool), nia_diagnostic::Diagnostic> {
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| error.diagnostic())?;
    let fingerprint =
        fingerprint::compiler_builtins_fingerprint(&symbols, options, &target_identity);
    if let Some(bytes) =
        load_object_work_product(cache, &CodegenUnitKey::CompilerBuiltins, fingerprint)
    {
        return Ok((
            LlvmObjectModuleOutput {
                unit: CodegenUnitId::CompilerBuiltins,
                key: CodegenUnitKey::CompilerBuiltins,
                fingerprint,
                name: "nia.compiler_builtins".to_string(),
                bytes,
            },
            true,
        ));
    }
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            &target_identity,
            llvm_optimization_level(options.optimization.level),
        )
    })
    .map_err(|error| error.diagnostic())?;
    let bytes = compiler_builtins::emit_object(&target, symbols)?;
    publish_object_work_product(
        cache,
        &CodegenUnitKey::CompilerBuiltins,
        fingerprint,
        &bytes,
    );
    Ok((
        LlvmObjectModuleOutput {
            unit: CodegenUnitId::CompilerBuiltins,
            key: CodegenUnitKey::CompilerBuiltins,
            fingerprint,
            name: "nia.compiler_builtins".to_string(),
            bytes,
        },
        false,
    ))
}

fn load_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprint: CodegenUnitFingerprint,
) -> Option<Vec<u8>> {
    cache?.load(key, fingerprint).ok().flatten()
}

fn publish_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprint: CodegenUnitFingerprint,
    bytes: &[u8],
) {
    if let Some(cache) = cache {
        let _ = cache.publish(key, fingerprint, bytes);
    }
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
