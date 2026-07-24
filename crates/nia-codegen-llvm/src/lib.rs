// SPDX-License-Identifier: GPL-3.0-or-later
mod backend_validate;
mod compiler_builtins;
mod declaration_membership;
mod fingerprint;
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;
mod work_product;

use std::sync::Arc;

use backend_validate::{
    validate_backend_declaration_module, validate_backend_partition_declarations,
    validate_backend_partition_definitions,
};
use declaration_membership::CodegenDeclarationMembership;
use module_codegen::ModuleCodegen;
use nia_backend_ir::CodegenPartition;
pub use nia_backend_ir::{
    CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInput,
    IncrementalLinkInputs,
};
use nia_backend_lower::BackendLowering;
use nia_ids::ModuleId;
use nia_llvm::{Context, OptimizationLevel as LlvmOptimizationLevel, target::TargetMachine};
use nia_opt::NiaOptimizationLevel;
use nia_query::QuerySession;
use nia_ty::TypeStore;
pub use output::{
    LlvmCodegenOptions, LlvmCodegenOutput, LlvmModuleOutput, LlvmObjectOutput, NativeObject,
};
use program_index::ProgramIndex;
pub use work_product::{
    CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet, ObjectWorkProductCache,
    ObjectWorkProductInvalidation, ObjectWorkProductLookup,
};

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
    let mut tasks = partitions
        .iter()
        .cloned()
        .map(LlvmIrTask::Partition)
        .collect::<Vec<_>>();
    tasks.extend(
        declaration_only_modules(&index, &partitions)
            .into_iter()
            .map(LlvmIrTask::DeclarationModule),
    );
    let worker_lanes = codegen_worker_lanes(session, tasks.len());
    let outcomes = session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            move || match task {
                LlvmIrTask::Partition(partition) => {
                    emit_llvm_ir_partition(partition, index, options).map(Some)
                }
                LlvmIrTask::DeclarationModule(module_id) => {
                    validate_declaration_module(module_id, &index).map(|()| None)
                }
            }
        }),
        nia_query::llvm_memory_task_capacity(),
    );
    let mut outputs = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(Some(output)) => outputs.push(output),
            Ok(None) => {}
            Err(partition_diagnostics) => diagnostics.extend(partition_diagnostics),
        }
    }
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", outputs.len() as u64);
        nia_timing::emit_counter("llvm.worker_lanes", worker_lanes as u64);
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
    let builtin_symbols = compiler_builtins::required_symbols(index.program(), &index);
    let mut tasks = partitions
        .iter()
        .cloned()
        .map(NativeCodegenTask::Partition)
        .collect::<Vec<_>>();
    tasks.extend(
        declaration_only_modules(&index, &partitions)
            .into_iter()
            .map(NativeCodegenTask::DeclarationModule),
    );
    if builtin_symbols.any() {
        tasks.push(NativeCodegenTask::CompilerBuiltins(builtin_symbols));
    }
    let worker_lanes = codegen_worker_lanes(session, tasks.len());
    let outcomes = session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            let cache = cache.clone();
            move || match task {
                NativeCodegenTask::Partition(partition) => {
                    emit_native_object_partition(partition, index, options, cache.as_deref())
                        .map(Some)
                }
                NativeCodegenTask::DeclarationModule(module_id) => {
                    validate_declaration_module(module_id, &index).map(|()| None)
                }
                NativeCodegenTask::CompilerBuiltins(symbols) => {
                    emit_compiler_builtins_object(symbols, options, cache.as_deref())
                        .map(Some)
                        .map_err(|diagnostic| vec![diagnostic])
                }
            }
        }),
        nia_query::llvm_memory_task_capacity(),
    );
    let mut outputs = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    let mut reuse_counts = ObjectReuseCounts::default();
    for outcome in outcomes {
        match outcome {
            Ok(Some((output, reuse))) => {
                reuse_counts.record(reuse);
                outputs.push(output);
            }
            Ok(None) => {}
            Err(task_diagnostics) => diagnostics.extend(task_diagnostics),
        }
    }
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", outputs.len() as u64);
        nia_timing::emit_counter("llvm.worker_lanes", worker_lanes as u64);
        reuse_counts.emit();
    }
    LlvmObjectOutput {
        link_inputs: IncrementalLinkInputs::new(outputs),
        diagnostics,
    }
}

enum NativeCodegenTask {
    Partition(CodegenPartition),
    DeclarationModule(ModuleId),
    CompilerBuiltins(compiler_builtins::CompilerBuiltinSymbols),
}

enum LlvmIrTask {
    Partition(CodegenPartition),
    DeclarationModule(ModuleId),
}

fn declaration_only_modules(
    index: &ProgramIndex,
    partitions: &[CodegenPartition],
) -> Vec<ModuleId> {
    if !partitions.is_empty() {
        return Vec::new();
    }
    index
        .program()
        .modules
        .iter()
        .map(|module| module.id)
        .collect()
}

fn validate_declaration_module(
    module_id: ModuleId,
    index: &ProgramIndex,
) -> Result<(), Vec<nia_diagnostic::Diagnostic>> {
    let module = index
        .module(module_id)
        .expect("declaration validation task references published module");
    let diagnostics = validate_backend_declaration_module(module, index);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn codegen_worker_lanes(session: &QuerySession, task_count: usize) -> usize {
    task_count
        .min(session.executor_parallelism())
        .min(nia_query::llvm_memory_task_capacity())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectReuse {
    Hit,
    Miss(ObjectReuseMiss),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectReuseMiss {
    Disabled,
    NotFound,
    Invalidated(ObjectWorkProductInvalidation),
    Corrupt,
    ReadError,
}

#[derive(Debug, Default)]
struct ObjectReuseCounts {
    hits: u64,
    disabled: u64,
    not_found: u64,
    invalidated: u64,
    invalidated_policy: u64,
    invalidated_definition: u64,
    invalidated_declarations: u64,
    invalidated_target: u64,
    corrupt: u64,
    read_error: u64,
}

impl ObjectReuseCounts {
    fn record(&mut self, reuse: ObjectReuse) {
        match reuse {
            ObjectReuse::Hit => self.hits += 1,
            ObjectReuse::Miss(ObjectReuseMiss::Disabled) => self.disabled += 1,
            ObjectReuse::Miss(ObjectReuseMiss::NotFound) => self.not_found += 1,
            ObjectReuse::Miss(ObjectReuseMiss::Invalidated(reasons)) => {
                self.invalidated += 1;
                self.invalidated_policy += u64::from(reasons.policy);
                self.invalidated_definition += u64::from(reasons.definition);
                self.invalidated_declarations += u64::from(reasons.declarations);
                self.invalidated_target += u64::from(reasons.target);
            }
            ObjectReuse::Miss(ObjectReuseMiss::Corrupt) => self.corrupt += 1,
            ObjectReuse::Miss(ObjectReuseMiss::ReadError) => self.read_error += 1,
        }
    }

    fn emit(&self) {
        let misses =
            self.disabled + self.not_found + self.invalidated + self.corrupt + self.read_error;
        nia_timing::emit_counter("llvm.object_reuse_hits", self.hits);
        nia_timing::emit_counter("llvm.object_reuse_misses", misses);
        nia_timing::emit_counter("llvm.object_reuse_miss_disabled", self.disabled);
        nia_timing::emit_counter("llvm.object_reuse_miss_not_found", self.not_found);
        nia_timing::emit_counter("llvm.object_reuse_miss_invalidated", self.invalidated);
        nia_timing::emit_counter("llvm.object_invalidation_policy", self.invalidated_policy);
        nia_timing::emit_counter(
            "llvm.object_invalidation_definition",
            self.invalidated_definition,
        );
        nia_timing::emit_counter(
            "llvm.object_invalidation_declarations",
            self.invalidated_declarations,
        );
        nia_timing::emit_counter("llvm.object_invalidation_target", self.invalidated_target);
        nia_timing::emit_counter("llvm.object_reuse_miss_corrupt", self.corrupt);
        nia_timing::emit_counter("llvm.object_reuse_miss_read_error", self.read_error);
    }
}

fn emit_llvm_ir_partition(
    partition: CodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
) -> Result<LlvmModuleOutput, Vec<nia_diagnostic::Diagnostic>> {
    let module = index.program().module_for_partition(&partition);
    let diagnostics = validate_backend_partition_definitions(&partition, &index);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let declarations = CodegenDeclarationMembership::build(&partition, &index);
    let diagnostics = validate_backend_partition_declarations(&declarations, &index);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let fingerprints = fingerprint::source_unit_fingerprint(
        &partition,
        &declarations,
        &index,
        options,
        fingerprint::ArtifactTarget::LlvmIr,
    );
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, &partition, &declarations, &index, options)
        })
        .map_err(|diagnostic| vec![diagnostic])?;
    let ir = codegen.emit_ir().map_err(|diagnostic| vec![diagnostic])?;
    Ok(LlvmModuleOutput {
        unit: partition.id,
        key: partition.key,
        fingerprint: fingerprints.fingerprint,
        name: module.name.clone(),
        ir,
    })
}

fn emit_native_object_partition(
    partition: CodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(IncrementalLinkInput<NativeObject>, ObjectReuse), Vec<nia_diagnostic::Diagnostic>> {
    let diagnostics = validate_backend_partition_definitions(&partition, &index);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| vec![error.diagnostic()])?;
    let declarations = CodegenDeclarationMembership::build(&partition, &index);
    let diagnostics = validate_backend_partition_declarations(&declarations, &index);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let fingerprints = fingerprint::source_unit_fingerprint(
        &partition,
        &declarations,
        &index,
        options,
        fingerprint::ArtifactTarget::NativeObject(&target_identity),
    );
    let module = index.program().module_for_partition(&partition);
    let lookup = load_object_work_product(cache, &partition.key, fingerprints);
    if let ObjectReuseLookup::Hit(bytes) = lookup {
        return Ok((
            IncrementalLinkInput {
                key: partition.key,
                fingerprint: fingerprints.fingerprint,
                object: NativeObject {
                    unit: partition.id,
                    name: module.name.clone(),
                    bytes,
                },
            },
            ObjectReuse::Hit,
        ));
    }
    let ObjectReuseLookup::Miss(miss) = lookup else {
        unreachable!("object cache hit returned before codegen")
    };
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            &target_identity,
            llvm_optimization_level(options.optimization.level),
        )
    })
    .map_err(|error| vec![error.diagnostic()])?;
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create);
    let mut codegen =
        time_codegen_module_stage(options.timings, "new_module", &module.name, || {
            ModuleCodegen::new(&context, module, &partition, &declarations, &index, options)
        })
        .map_err(|diagnostic| vec![diagnostic])?;
    target
        .configure_module(&codegen.module)
        .map_err(|error| vec![error.diagnostic()])?;
    let bytes = codegen
        .emit_object(&target)
        .map_err(|diagnostic| vec![diagnostic])?;
    publish_object_work_product(cache, &partition.key, fingerprints, &bytes);
    Ok((
        IncrementalLinkInput {
            key: partition.key,
            fingerprint: fingerprints.fingerprint,
            object: NativeObject {
                unit: partition.id,
                name: module.name.clone(),
                bytes,
            },
        },
        ObjectReuse::Miss(miss),
    ))
}

fn emit_compiler_builtins_object(
    symbols: compiler_builtins::CompilerBuiltinSymbols,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(IncrementalLinkInput<NativeObject>, ObjectReuse), nia_diagnostic::Diagnostic> {
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| error.diagnostic())?;
    let fingerprints =
        fingerprint::compiler_builtins_fingerprint(&symbols, options, &target_identity);
    let lookup = load_object_work_product(cache, &CodegenUnitKey::CompilerBuiltins, fingerprints);
    if let ObjectReuseLookup::Hit(bytes) = lookup {
        return Ok((
            IncrementalLinkInput {
                key: CodegenUnitKey::CompilerBuiltins,
                fingerprint: fingerprints.fingerprint,
                object: NativeObject {
                    unit: CodegenUnitId::CompilerBuiltins,
                    name: "nia.compiler_builtins".to_string(),
                    bytes,
                },
            },
            ObjectReuse::Hit,
        ));
    }
    let ObjectReuseLookup::Miss(miss) = lookup else {
        unreachable!("object cache hit returned before codegen")
    };
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
        fingerprints,
        &bytes,
    );
    Ok((
        IncrementalLinkInput {
            key: CodegenUnitKey::CompilerBuiltins,
            fingerprint: fingerprints.fingerprint,
            object: NativeObject {
                unit: CodegenUnitId::CompilerBuiltins,
                name: "nia.compiler_builtins".to_string(),
                bytes,
            },
        },
        ObjectReuse::Miss(miss),
    ))
}

enum ObjectReuseLookup {
    Hit(Vec<u8>),
    Miss(ObjectReuseMiss),
}

fn load_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
) -> ObjectReuseLookup {
    let Some(cache) = cache else {
        return ObjectReuseLookup::Miss(ObjectReuseMiss::Disabled);
    };
    match cache.load(key, fingerprints) {
        Ok(ObjectWorkProductLookup::Hit(bytes)) => ObjectReuseLookup::Hit(bytes),
        Ok(ObjectWorkProductLookup::NotFound) => ObjectReuseLookup::Miss(ObjectReuseMiss::NotFound),
        Ok(ObjectWorkProductLookup::Invalidated(reasons)) => {
            ObjectReuseLookup::Miss(ObjectReuseMiss::Invalidated(reasons))
        }
        Ok(ObjectWorkProductLookup::Corrupt) => ObjectReuseLookup::Miss(ObjectReuseMiss::Corrupt),
        Err(_) => ObjectReuseLookup::Miss(ObjectReuseMiss::ReadError),
    }
}

fn publish_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
) {
    if let Some(cache) = cache {
        let _ = cache.publish(key, fingerprints, bytes);
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
            link_inputs: IncrementalLinkInputs::default(),
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
