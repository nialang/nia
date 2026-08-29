// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated backend IR emission through LLVM.
//!
//! This crate is the LLVM boundary of the compiler. Callers provide a complete
//! [`BackendLowering`] together with the matching [`TypeStore`]; codegen first
//! validates partition definitions and declarations, then creates LLVM modules
//! or native objects. Malformed backend IR is reported as diagnostics before it
//! reaches the unsafe LLVM wrapper layer.
//!
//! The readiness emitters expose the same pipeline incrementally. Modules may
//! be published as backend finalization completes, while partitions are emitted
//! only after all declaration owners they depend on are visible. `finish`
//! requires every module in the store to have been published and returns output
//! sorted by stable codegen-unit key.
mod backend_validate;
mod compiler_builtins;
mod declaration_membership;
mod fingerprint;
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;
mod readiness;
mod work_product;

use std::sync::Arc;

use backend_validate::{
    validate_backend_declaration_module, validate_backend_partition_declarations,
    validate_backend_program, validate_native_backend_program,
};
use module_codegen::ModuleCodegen;
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
use readiness::{
    CodegenPartitionPreparation, CodegenReadinessCoordinator, PreparedCodegenPartition,
};
pub use work_product::{
    CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet, ObjectWorkProductCache,
    ObjectWorkProductInvalidation, ObjectWorkProductLookup,
};

type LlvmIrReadinessOutcome = (
    CodegenUnitKey,
    Result<LlvmModuleOutput, Vec<nia_diagnostic::Diagnostic>>,
);
type LlvmNativeObjectReadinessOutcome = (
    CodegenUnitKey,
    Result<(IncrementalLinkInput<NativeObject>, ObjectReuse), Vec<nia_diagnostic::Diagnostic>>,
);

/// Incrementally emits textual LLVM IR from finalized backend modules.
///
/// Create one emitter for one [`nia_backend_ir::BackendModuleStore`], publish
/// each readiness token produced by that store exactly once, then call
/// [`finish`](Self::finish). Partitions can execute before unrelated modules
/// finish, but validation waits until every declaration required by a partition
/// is available.
pub struct LlvmIrReadinessEmitter<'session> {
    coordinator: CodegenReadinessCoordinator,
    options: LlvmCodegenOptions,
    outputs: Vec<LlvmModuleOutput>,
    partition_diagnostics: Vec<(CodegenUnitKey, Vec<nia_diagnostic::Diagnostic>)>,
    partition_count: usize,
    tasks: nia_query::QueryTaskPool<'session, LlvmIrReadinessOutcome>,
}

/// Incrementally emits native objects from finalized backend modules.
///
/// This follows the same publication contract as [`LlvmIrReadinessEmitter`]
/// and additionally performs incremental object-cache lookup and publication.
/// Cache failures become diagnostics for the affected codegen unit rather than
/// suppressing validation of other units.
pub struct LlvmNativeObjectReadinessEmitter<'session> {
    coordinator: CodegenReadinessCoordinator,
    options: LlvmCodegenOptions,
    cache: Option<Arc<dyn ObjectWorkProductCache>>,
    outputs: Vec<IncrementalLinkInput<NativeObject>>,
    partition_diagnostics: Vec<(CodegenUnitKey, Vec<nia_diagnostic::Diagnostic>)>,
    reuse_counts: ObjectReuseCounts,
    partition_count: usize,
    tasks: nia_query::QueryTaskPool<'session, LlvmNativeObjectReadinessOutcome>,
}

impl<'session> LlvmNativeObjectReadinessEmitter<'session> {
    /// Starts native-object emission for a module store and its owner index.
    ///
    /// `modules`, `type_store`, and `owners` must describe the same backend
    /// program. The emitter retains them until [`finish`](Self::finish).
    pub fn new(
        modules: Arc<nia_backend_ir::BackendModuleStore>,
        type_store: Arc<TypeStore>,
        owners: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
        options: LlvmCodegenOptions,
        cache: Option<Arc<dyn ObjectWorkProductCache>>,
        session: &'session QuerySession,
    ) -> Self {
        Self {
            coordinator: CodegenReadinessCoordinator::new(modules, type_store, owners),
            options,
            cache,
            outputs: Vec::new(),
            partition_diagnostics: Vec::new(),
            reuse_counts: ObjectReuseCounts::default(),
            partition_count: 0,
            tasks: session.task_pool(nia_query::llvm_memory_task_capacity()),
        }
    }

    /// Publishes one finalized module and schedules every newly ready partition.
    ///
    /// Readiness tokens are single-use ownership events from the associated
    /// module store. Publishing the same module twice is an internal contract
    /// violation.
    pub fn publish(&mut self, ready: nia_backend_ir::BackendModuleReady) {
        for preparation in self.coordinator.publish(ready.module_id()) {
            self.partition_count += 1;
            match preparation {
                CodegenPartitionPreparation::Ready(prepared) => {
                    let key = prepared.partition.key.clone();
                    let index = Arc::clone(&self.coordinator.index);
                    let options = self.options;
                    let cache = self.cache.clone();
                    self.tasks.submit(move || {
                        let outcome = emit_native_object_partition(
                            prepared,
                            index,
                            options,
                            cache.as_deref(),
                        );
                        (key, outcome)
                    });
                }
                CodegenPartitionPreparation::Invalid {
                    partition,
                    diagnostics,
                } => self
                    .partition_diagnostics
                    .push((partition.key, diagnostics)),
            }
        }
    }

    /// Waits for scheduled work and returns deterministically ordered objects.
    ///
    /// This must be called only after every module readiness token has been
    /// published. Invalid units are omitted from `link_inputs` and represented
    /// in the returned diagnostics.
    pub fn finish(mut self) -> LlvmObjectOutput {
        let index = self.coordinator.finish();
        let builtin_symbols = compiler_builtins::required_symbols(&index);
        let program_diagnostics = validate_native_backend_program(&index, builtin_symbols);
        let worker_lanes = self.partition_count.min(self.tasks.capacity());
        let task_outcomes = self.tasks.finish();
        for (key, outcome) in task_outcomes {
            match outcome {
                Ok((output, reuse)) => {
                    self.reuse_counts.record(reuse);
                    self.outputs.push(output);
                }
                Err(diagnostics) => self.partition_diagnostics.push((key, diagnostics)),
            }
        }
        let has_partitions = self.partition_count != 0;
        let mut declaration_diagnostics = Vec::new();
        if !has_partitions && program_diagnostics.is_empty() {
            for module_id in index.module_ids() {
                if let Err(diagnostics) = validate_declaration_module(*module_id, &index) {
                    declaration_diagnostics.extend(diagnostics);
                }
            }
        }
        if program_diagnostics.is_empty() && builtin_symbols.any() {
            match emit_compiler_builtins_object(
                builtin_symbols,
                self.options,
                self.cache.as_deref(),
            ) {
                Ok((output, reuse)) => {
                    self.reuse_counts.record(reuse);
                    self.outputs.push(output);
                }
                Err(diagnostic) => self
                    .partition_diagnostics
                    .push((CodegenUnitKey::CompilerBuiltins, vec![diagnostic])),
            }
        }
        if !program_diagnostics.is_empty() {
            self.outputs.clear();
        }
        self.outputs
            .sort_unstable_by(|left, right| left.key.cmp(&right.key));
        self.partition_diagnostics
            .sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut diagnostics = self
            .partition_diagnostics
            .into_iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .collect::<Vec<_>>();
        diagnostics.extend(declaration_diagnostics);
        diagnostics.extend(program_diagnostics);
        if self.options.timings.enabled() {
            nia_timing::emit_counter("llvm.units", self.outputs.len() as u64);
            nia_timing::emit_counter(
                "llvm.worker_lanes",
                worker_lanes.max(usize::from(!index.module_ids().is_empty())) as u64,
            );
            nia_timing::emit_counter("llvm.ready_task_submissions", self.partition_count as u64);
            self.reuse_counts.emit();
        }
        LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::new(self.outputs),
            diagnostics,
        }
    }
}

impl<'session> LlvmIrReadinessEmitter<'session> {
    /// Starts textual IR emission for a module store and its owner index.
    ///
    /// `modules`, `type_store`, and `owners` must describe the same backend
    /// program. The emitter retains them until [`finish`](Self::finish).
    pub fn new(
        modules: Arc<nia_backend_ir::BackendModuleStore>,
        type_store: Arc<TypeStore>,
        owners: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
        options: LlvmCodegenOptions,
        session: &'session QuerySession,
    ) -> Self {
        Self {
            coordinator: CodegenReadinessCoordinator::new(modules, type_store, owners),
            options,
            outputs: Vec::new(),
            partition_diagnostics: Vec::new(),
            partition_count: 0,
            tasks: session.task_pool(nia_query::llvm_memory_task_capacity()),
        }
    }

    /// Publishes one finalized module and schedules every newly ready partition.
    ///
    /// Readiness tokens are single-use ownership events from the associated
    /// module store. Publishing the same module twice is an internal contract
    /// violation.
    pub fn publish(&mut self, ready: nia_backend_ir::BackendModuleReady) {
        for preparation in self.coordinator.publish(ready.module_id()) {
            self.partition_count += 1;
            match preparation {
                CodegenPartitionPreparation::Ready(prepared) => {
                    let key = prepared.partition.key.clone();
                    let index = Arc::clone(&self.coordinator.index);
                    let options = self.options;
                    self.tasks.submit(move || {
                        let outcome = emit_llvm_ir_partition(prepared, index, options);
                        (key, outcome)
                    });
                }
                CodegenPartitionPreparation::Invalid {
                    partition,
                    diagnostics,
                } => self
                    .partition_diagnostics
                    .push((partition.key, diagnostics)),
            }
        }
    }

    /// Waits for scheduled work and returns deterministically ordered IR units.
    ///
    /// This must be called only after every module readiness token has been
    /// published. Invalid units are omitted from `modules` and represented in
    /// the returned diagnostics.
    pub fn finish(mut self) -> LlvmCodegenOutput {
        let index = self.coordinator.finish();
        let program_diagnostics = validate_backend_program(&index);
        let worker_lanes = self.partition_count.min(self.tasks.capacity());
        let task_outcomes = self.tasks.finish();
        for (key, outcome) in task_outcomes {
            match outcome {
                Ok(output) => self.outputs.push(output),
                Err(diagnostics) => self.partition_diagnostics.push((key, diagnostics)),
            }
        }
        let mut declaration_diagnostics = Vec::new();
        if self.partition_count == 0 && program_diagnostics.is_empty() {
            for module_id in index.module_ids() {
                if let Err(diagnostics) = validate_declaration_module(*module_id, &index) {
                    declaration_diagnostics.extend(diagnostics);
                }
            }
        }
        if !program_diagnostics.is_empty() {
            self.outputs.clear();
        }
        self.outputs
            .sort_unstable_by(|left, right| left.key.cmp(&right.key));
        self.partition_diagnostics
            .sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut diagnostics = self
            .partition_diagnostics
            .into_iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .collect::<Vec<_>>();
        diagnostics.extend(declaration_diagnostics);
        diagnostics.extend(program_diagnostics);
        if self.options.timings.enabled() {
            nia_timing::emit_counter("llvm.units", self.outputs.len() as u64);
            nia_timing::emit_counter(
                "llvm.worker_lanes",
                worker_lanes.max(usize::from(!index.module_ids().is_empty())) as u64,
            );
            nia_timing::emit_counter("llvm.ready_task_submissions", self.partition_count as u64);
        }
        LlvmCodegenOutput {
            modules: self.outputs,
            diagnostics,
        }
    }
}

/// Validates and emits textual LLVM IR with default codegen options.
///
/// Backend diagnostics already present in `lowering` remain owned by the
/// lowering caller; this function reports only failures discovered at the
/// backend-IR/LLVM boundary.
pub fn emit_llvm_ir(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
) -> LlvmCodegenOutput {
    emit_llvm_ir_with_options(lowering, type_store, session, LlvmCodegenOptions::default())
}

/// Validates and emits textual LLVM IR for every codegen partition.
///
/// Independent partitions run through the query session's bounded task pool.
/// A failed partition does not discard valid output from other partitions.
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
    let module_store = lowering.program.module_store();
    let owners = Arc::clone(&lowering.owner_directory);
    let (index, preparations) = time_codegen_stage(timings, "llvm_codegen.program_index", || {
        prepare_complete_codegen(module_store, type_store, owners)
    });
    let program_diagnostics = validate_backend_program(&index);
    if !program_diagnostics.is_empty() {
        return LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: program_diagnostics,
        };
    }
    let has_partitions = !preparations.is_empty();
    let mut tasks = preparations
        .into_iter()
        .map(|preparation| LlvmIrTask::Partition(Box::new(preparation)))
        .collect::<Vec<_>>();
    tasks.extend(
        declaration_only_modules(&index, has_partitions)
            .into_iter()
            .map(LlvmIrTask::DeclarationModule),
    );
    let worker_lanes = codegen_worker_lanes(session, tasks.len());
    let outcomes = session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            move || match task {
                LlvmIrTask::Partition(preparation) => match *preparation {
                    CodegenPartitionPreparation::Ready(prepared) => {
                        emit_llvm_ir_partition(prepared, index, options).map(Some)
                    }
                    CodegenPartitionPreparation::Invalid { diagnostics, .. } => Err(diagnostics),
                },
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

/// Validates backend IR and emits linkable native object work products.
///
/// When `cache` is present, lookup uses the complete policy, definition,
/// declaration, and target fingerprint set. Invalid or corrupt entries are
/// regenerated; I/O failures are returned as diagnostics for their unit.
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
    let module_store = lowering.program.module_store();
    let owners = Arc::clone(&lowering.owner_directory);
    let (index, preparations) = time_codegen_stage(timings, "llvm_codegen.program_index", || {
        prepare_complete_codegen(module_store, type_store, owners)
    });
    let builtin_symbols = compiler_builtins::required_symbols(&index);
    let program_diagnostics = validate_native_backend_program(&index, builtin_symbols);
    if !program_diagnostics.is_empty() {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::new(Vec::new()),
            diagnostics: program_diagnostics,
        };
    }
    let has_partitions = !preparations.is_empty();
    let mut tasks = preparations
        .into_iter()
        .map(|preparation| NativeCodegenTask::Partition(Box::new(preparation)))
        .collect::<Vec<_>>();
    tasks.extend(
        declaration_only_modules(&index, has_partitions)
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
                NativeCodegenTask::Partition(preparation) => match *preparation {
                    CodegenPartitionPreparation::Ready(prepared) => {
                        emit_native_object_partition(prepared, index, options, cache.as_deref())
                            .map(Some)
                    }
                    CodegenPartitionPreparation::Invalid { diagnostics, .. } => Err(diagnostics),
                },
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
    Partition(Box<CodegenPartitionPreparation>),
    DeclarationModule(ModuleId),
    CompilerBuiltins(compiler_builtins::CompilerBuiltinSymbols),
}

enum LlvmIrTask {
    Partition(Box<CodegenPartitionPreparation>),
    DeclarationModule(ModuleId),
}

fn declaration_only_modules(index: &ProgramIndex, has_partitions: bool) -> Vec<ModuleId> {
    if has_partitions {
        return Vec::new();
    }
    index.module_ids().to_vec()
}

fn prepare_complete_codegen(
    modules: Arc<nia_backend_ir::BackendModuleStore>,
    type_store: Arc<TypeStore>,
    owners: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
) -> (Arc<ProgramIndex>, Vec<CodegenPartitionPreparation>) {
    let module_ids = modules.module_ids().to_vec();
    let mut coordinator = CodegenReadinessCoordinator::new(modules, type_store, owners);
    let mut preparations = Vec::new();
    for module_id in module_ids {
        preparations.extend(coordinator.publish(module_id));
    }
    preparations.sort_unstable_by(|left, right| left.key().cmp(right.key()));
    (coordinator.finish(), preparations)
}

fn validate_declaration_module(
    module_id: ModuleId,
    index: &ProgramIndex,
) -> Result<(), Vec<nia_diagnostic::Diagnostic>> {
    let Some(module) = index.module(module_id) else {
        return Err(vec![nia_diagnostic::Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("backend declaration validation references missing module {module_id:?}"),
        )]);
    };
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
    prepared: PreparedCodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
) -> Result<LlvmModuleOutput, Vec<nia_diagnostic::Diagnostic>> {
    let PreparedCodegenPartition {
        partition,
        declarations,
    } = prepared;
    let module = index.module_for_partition(&partition);
    let diagnostics =
        validate_backend_partition_declarations(&declarations, &index, module.layouts.target);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let fingerprints = fingerprint::source_unit_fingerprint(
        &partition,
        &declarations,
        &index,
        options,
        fingerprint::ArtifactTarget::LlvmIr,
    )?;
    let memory_permit = nia_query::acquire_llvm_memory_permit();
    record_memory_permit(options.timings, memory_permit.waited());
    let context =
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create)
            .map_err(|error| vec![error.diagnostic()])?;
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
    prepared: PreparedCodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(IncrementalLinkInput<NativeObject>, ObjectReuse), Vec<nia_diagnostic::Diagnostic>> {
    let PreparedCodegenPartition {
        partition,
        declarations,
    } = prepared;
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| vec![error.diagnostic()])?;
    let module = index.module_for_partition(&partition);
    let diagnostics =
        validate_backend_partition_declarations(&declarations, &index, module.layouts.target);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let fingerprints = fingerprint::source_unit_fingerprint(
        &partition,
        &declarations,
        &index,
        options,
        fingerprint::ArtifactTarget::NativeObject(&target_identity),
    )?;
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
        time_codegen_module_stage(options.timings, "context", &module.name, Context::create)
            .map_err(|error| vec![error.diagnostic()])?;
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
/// LLVM's speed-oriented optimization tier after mapping from Nia policy.
pub enum LlvmCodegenOptimizationLevel {
    /// Disable LLVM optimization passes.
    None,
    /// Run LLVM's lightweight optimization pipeline.
    Less,
    /// Run LLVM's standard optimization pipeline.
    Default,
    /// Run LLVM's most aggressive speed optimization pipeline.
    Aggressive,
}

impl LlvmCodegenOptimizationLevel {
    /// Returns the stable diagnostic and fingerprint name for this tier.
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
/// Code-size preference passed independently from LLVM's speed tier.
pub enum LlvmCodegenSizePolicy {
    /// Do not request size-specific optimization.
    Default,
    /// Prefer smaller output while retaining the standard speed tier.
    Small,
    /// Minimize output size more aggressively.
    Tiny,
}

impl LlvmCodegenSizePolicy {
    /// Returns the stable diagnostic and fingerprint name for this policy.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Small => "small",
            Self::Tiny => "tiny",
        }
    }
}

/// Maps a Nia optimization level to LLVM's speed-oriented pass tier.
///
/// Size modes are intentionally split: `Os` uses the default speed tier and
/// `Oz` uses the lighter tier, while [`llvm_codegen_size_policy`] carries the
/// explicit size preference.
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

/// Maps a Nia optimization level to LLVM's independent size preference.
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
