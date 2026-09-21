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
mod orchestration;
mod output;
mod program_index;
mod readiness;
mod work_product;

use std::{collections::HashMap, sync::Arc};

use backend_validate::{
    validate_backend_declaration_module, validate_backend_partition_declarations,
    validate_backend_program, validate_native_backend_program,
};
use module_codegen::ModuleCodegen;
pub use nia_backend_ir::{
    BackendModuleReady, CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey,
    IncrementalLinkInput, IncrementalLinkInputs,
};
use nia_backend_lower::BackendLowering;
use nia_ids::ModuleId;
use nia_llvm::{
    Context, OptimizationLevel as LlvmOptimizationLevel,
    target::{TargetMachine, TargetMachineIdentity},
};
use nia_opt::NiaOptimizationLevel;
use nia_query::{FingerprintDomain, QueryFingerprintBuilder, QuerySession};
use nia_ty::TypeStore;
pub use output::{
    FullLtoCodegenConfig, LlvmCodegenOptions, LlvmCodegenOutput, LlvmLtoModuleOutput,
    LlvmModuleOutput, LlvmObjectOutput, LtoMode, LtoModule, LtoPreLinkConfig, NativeObject,
    ThinLtoCodegenConfig,
};
use program_index::ProgramIndex;
use readiness::{
    CodegenPartitionPreparation, CodegenReadinessCoordinator, PreparedCodegenPartition,
};
pub use work_product::{
    CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet, CodegenWorkProductInvalidation,
    CodegenWorkProductLookup, LtoModuleWorkProductCache, ObjectWorkProductCache,
};
use work_product::{WorkProductReuse, WorkProductReuseCounts, WorkProductReuseMiss};

type LlvmIrReadinessOutcome = (
    CodegenUnitKey,
    Result<LlvmModuleOutput, Vec<nia_diagnostic::Diagnostic>>,
);
type LlvmNativeObjectReadinessOutcome = (
    CodegenUnitKey,
    Result<(IncrementalLinkInput<NativeObject>, WorkProductReuse), Vec<nia_diagnostic::Diagnostic>>,
);
type LlvmLtoReadinessOutcome = (
    CodegenUnitKey,
    Result<(LtoModule, WorkProductReuse), Vec<nia_diagnostic::Diagnostic>>,
);

const THIN_LTO_BACKEND_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.llvm.thin-lto-backend");
const FULL_LTO_BACKEND_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.llvm.full-lto-backend");

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
    internal_diagnostics: Vec<nia_diagnostic::Diagnostic>,
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
    internal_diagnostics: Vec<nia_diagnostic::Diagnostic>,
    reuse_counts: WorkProductReuseCounts,
    partition_count: usize,
    tasks: nia_query::QueryTaskPool<'session, LlvmNativeObjectReadinessOutcome>,
}

/// Incrementally emits LTO pre-link modules from finalized backend modules.
///
/// This preserves the native-object readiness contract while stopping before
/// whole-program coordination. Callers must submit the complete returned module
/// set to the coordinator selected by [`LtoMode`].
pub struct LlvmLtoReadinessEmitter<'session> {
    coordinator: CodegenReadinessCoordinator,
    pre_link: LtoPreLinkConfig,
    options: LlvmCodegenOptions,
    cache: Option<Arc<dyn LtoModuleWorkProductCache>>,
    target_identity: Option<Arc<TargetMachineIdentity>>,
    outputs: Vec<LtoModule>,
    partition_diagnostics: Vec<(CodegenUnitKey, Vec<nia_diagnostic::Diagnostic>)>,
    internal_diagnostics: Vec<nia_diagnostic::Diagnostic>,
    reuse_counts: WorkProductReuseCounts,
    partition_count: usize,
    tasks: nia_query::QueryTaskPool<'session, LlvmLtoReadinessOutcome>,
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
    ) -> nia_ice::IceResult<Self> {
        Ok(Self {
            coordinator: CodegenReadinessCoordinator::new(modules, type_store, owners),
            options,
            cache,
            outputs: Vec::new(),
            partition_diagnostics: Vec::new(),
            internal_diagnostics: Vec::new(),
            reuse_counts: WorkProductReuseCounts::default(),
            partition_count: 0,
            tasks: session.task_pool(nia_query::llvm_memory_task_capacity())?,
        })
    }

    /// Publishes one finalized module and schedules every newly ready partition.
    ///
    /// Readiness tokens are single-use ownership events from the associated
    /// module store. Publishing the same module twice is an internal contract
    /// violation.
    pub fn publish(&mut self, ready: nia_backend_ir::BackendModuleReady) -> nia_ice::IceResult<()> {
        for preparation in self.coordinator.publish(ready.module_id())? {
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
                        Ok((key, outcome))
                    })?;
                }
                CodegenPartitionPreparation::Invalid {
                    partition,
                    diagnostics,
                } => self
                    .partition_diagnostics
                    .push((partition.key, diagnostics)),
            }
        }
        Ok(())
    }

    /// Waits for scheduled work and returns deterministically ordered objects.
    ///
    /// This must be called only after every module readiness token has been
    /// published. Invalid units are omitted from `link_inputs` and represented
    /// in the returned diagnostics.
    pub fn finish(mut self) -> nia_ice::IceResult<LlvmObjectOutput> {
        let index = time_codegen_stage(self.options.timings, "llvm_finish.coordinator", || {
            self.coordinator.finish()
        })?;
        let builtin_symbols =
            time_codegen_stage(self.options.timings, "llvm_finish.builtin_symbols", || {
                compiler_builtins::required_symbols(&index)
            });
        let program_diagnostics = time_codegen_stage(
            self.options.timings,
            "llvm_finish.program_validation",
            || validate_native_backend_program(&index, builtin_symbols),
        );
        let worker_lanes = self.partition_count.min(self.tasks.capacity());
        let task_outcomes =
            match time_codegen_stage(self.options.timings, "llvm_finish.task_collection", || {
                self.tasks.finish()
            }) {
                Ok(outcomes) => outcomes,
                Err(ice) => {
                    self.internal_diagnostics
                        .push(nia_diagnostic::Diagnostic::from(ice));
                    Vec::new()
                }
            };
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
            time_codegen_stage(
                self.options.timings,
                "llvm_finish.declaration_validation",
                || {
                    for module_id in index.module_ids() {
                        if let Err(diagnostics) = validate_declaration_module(*module_id, &index) {
                            declaration_diagnostics.extend(diagnostics);
                        }
                    }
                },
            );
        }
        if program_diagnostics.is_empty() && builtin_symbols.any() {
            let builtin_result = time_codegen_stage(
                self.options.timings,
                "llvm_finish.compiler_builtins",
                || {
                    emit_compiler_builtins_object(
                        builtin_symbols,
                        self.options,
                        self.cache.as_deref(),
                    )
                },
            );
            match builtin_result {
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
        time_codegen_stage(self.options.timings, "llvm_finish.sort_outputs", || {
            self.outputs
                .sort_unstable_by(|left, right| left.key.cmp(&right.key));
            self.partition_diagnostics
                .sort_unstable_by(|left, right| left.0.cmp(&right.0));
        });
        let mut diagnostics = time_codegen_stage(
            self.options.timings,
            "llvm_finish.collect_diagnostics",
            || {
                self.partition_diagnostics
                    .into_iter()
                    .flat_map(|(_, diagnostics)| diagnostics)
                    .collect::<Vec<_>>()
            },
        );
        diagnostics.extend(declaration_diagnostics);
        diagnostics.extend(program_diagnostics);
        diagnostics.extend(self.internal_diagnostics);
        if self.options.timings.enabled() {
            nia_timing::emit_counter("llvm.units", self.outputs.len() as u64);
            nia_timing::emit_counter(
                "llvm.worker_lanes",
                worker_lanes.max(usize::from(!index.module_ids().is_empty())) as u64,
            );
            nia_timing::emit_counter("llvm.ready_task_submissions", self.partition_count as u64);
            self.reuse_counts.emit("object");
        }
        Ok(LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::new(self.outputs)?,
            diagnostics,
        })
    }
}

impl<'session> LlvmLtoReadinessEmitter<'session> {
    /// Starts LTO pre-link emission for a module store and its owner index.
    pub fn new(
        modules: Arc<nia_backend_ir::BackendModuleStore>,
        type_store: Arc<TypeStore>,
        owners: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
        options: LlvmCodegenOptions,
        pre_link: LtoPreLinkConfig,
        cache: Option<Arc<dyn LtoModuleWorkProductCache>>,
        session: &'session QuerySession,
    ) -> nia_ice::IceResult<Self> {
        let (target_identity, internal_diagnostics) = match TargetMachine::native_identity() {
            Ok(identity) => (Some(Arc::new(identity)), Vec::new()),
            Err(error) => (None, vec![error.diagnostic()]),
        };
        Ok(Self {
            coordinator: CodegenReadinessCoordinator::new(modules, type_store, owners),
            pre_link,
            options,
            cache,
            target_identity,
            outputs: Vec::new(),
            partition_diagnostics: Vec::new(),
            internal_diagnostics,
            reuse_counts: WorkProductReuseCounts::default(),
            partition_count: 0,
            tasks: session.task_pool(nia_query::llvm_memory_task_capacity())?,
        })
    }

    /// Publishes one finalized module and schedules every newly ready pre-link unit.
    pub fn publish(&mut self, ready: nia_backend_ir::BackendModuleReady) -> nia_ice::IceResult<()> {
        for preparation in self.coordinator.publish(ready.module_id())? {
            self.partition_count += 1;
            match preparation {
                CodegenPartitionPreparation::Ready(prepared) => {
                    let Some(target_identity) = self.target_identity.as_ref().map(Arc::clone)
                    else {
                        continue;
                    };
                    let key = prepared.partition.key.clone();
                    let index = Arc::clone(&self.coordinator.index);
                    let options = self.options;
                    let pre_link = self.pre_link;
                    let cache = self.cache.clone();
                    self.tasks.submit(move || {
                        let outcome = emit_lto_partition(
                            prepared,
                            index,
                            options,
                            pre_link,
                            &target_identity,
                            cache.as_deref(),
                        );
                        Ok((key, outcome))
                    })?;
                }
                CodegenPartitionPreparation::Invalid {
                    partition,
                    diagnostics,
                } => self
                    .partition_diagnostics
                    .push((partition.key, diagnostics)),
            }
        }
        Ok(())
    }

    /// Waits for pre-link work and returns modules in stable codegen-unit order.
    pub fn finish(mut self) -> nia_ice::IceResult<LlvmLtoModuleOutput> {
        let index = time_codegen_stage(self.options.timings, "llvm_finish.coordinator", || {
            self.coordinator.finish()
        })?;
        let builtin_symbols =
            time_codegen_stage(self.options.timings, "llvm_finish.builtin_symbols", || {
                compiler_builtins::required_symbols(&index)
            });
        let program_diagnostics = time_codegen_stage(
            self.options.timings,
            "llvm_finish.program_validation",
            || validate_native_backend_program(&index, builtin_symbols),
        );
        let worker_lanes = self.partition_count.min(self.tasks.capacity());
        let task_outcomes =
            match time_codegen_stage(self.options.timings, "llvm_finish.task_collection", || {
                self.tasks.finish()
            }) {
                Ok(outcomes) => outcomes,
                Err(ice) => {
                    self.internal_diagnostics
                        .push(nia_diagnostic::Diagnostic::from(ice));
                    Vec::new()
                }
            };
        for (key, outcome) in task_outcomes {
            match outcome {
                Ok((output, reuse)) => {
                    self.reuse_counts.record(reuse);
                    self.outputs.push(output);
                }
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
        if program_diagnostics.is_empty()
            && builtin_symbols.any()
            && let Some(target_identity) = self.target_identity.as_deref()
        {
            match emit_compiler_builtins_lto_module(
                builtin_symbols,
                self.options,
                self.pre_link,
                target_identity,
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
        diagnostics.extend(self.internal_diagnostics);
        if self.options.timings.enabled() {
            nia_timing::emit_counter("llvm.units", self.outputs.len() as u64);
            nia_timing::emit_counter(
                "llvm.worker_lanes",
                worker_lanes.max(usize::from(!index.module_ids().is_empty())) as u64,
            );
            nia_timing::emit_counter("llvm.ready_task_submissions", self.partition_count as u64);
            self.reuse_counts.emit("lto_prelink");
        }
        Ok(LlvmLtoModuleOutput {
            pre_link: self.pre_link,
            target: self.target_identity.map(Arc::unwrap_or_clone),
            modules: self.outputs,
            linker_visible_symbols: lto_linker_visible_symbols(&index),
            diagnostics,
        })
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
    ) -> nia_ice::IceResult<Self> {
        Ok(Self {
            coordinator: CodegenReadinessCoordinator::new(modules, type_store, owners),
            options,
            outputs: Vec::new(),
            partition_diagnostics: Vec::new(),
            internal_diagnostics: Vec::new(),
            partition_count: 0,
            tasks: session.task_pool(nia_query::llvm_memory_task_capacity())?,
        })
    }

    /// Publishes one finalized module and schedules every newly ready partition.
    ///
    /// Readiness tokens are single-use ownership events from the associated
    /// module store. Publishing the same module twice is an internal contract
    /// violation.
    pub fn publish(&mut self, ready: nia_backend_ir::BackendModuleReady) -> nia_ice::IceResult<()> {
        for preparation in self.coordinator.publish(ready.module_id())? {
            self.partition_count += 1;
            match preparation {
                CodegenPartitionPreparation::Ready(prepared) => {
                    let key = prepared.partition.key.clone();
                    let index = Arc::clone(&self.coordinator.index);
                    let options = self.options;
                    self.tasks.submit(move || {
                        let outcome = emit_llvm_ir_partition(prepared, index, options);
                        Ok((key, outcome))
                    })?;
                }
                CodegenPartitionPreparation::Invalid {
                    partition,
                    diagnostics,
                } => self
                    .partition_diagnostics
                    .push((partition.key, diagnostics)),
            }
        }
        Ok(())
    }

    /// Waits for scheduled work and returns deterministically ordered IR units.
    ///
    /// This must be called only after every module readiness token has been
    /// published. Invalid units are omitted from `modules` and represented in
    /// the returned diagnostics.
    pub fn finish(mut self) -> nia_ice::IceResult<LlvmCodegenOutput> {
        let index = self.coordinator.finish()?;
        let program_diagnostics = validate_backend_program(&index);
        let worker_lanes = self.partition_count.min(self.tasks.capacity());
        let task_outcomes = match self.tasks.finish() {
            Ok(outcomes) => outcomes,
            Err(ice) => {
                self.internal_diagnostics
                    .push(nia_diagnostic::Diagnostic::from(ice));
                Vec::new()
            }
        };
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
        diagnostics.extend(self.internal_diagnostics);
        if self.options.timings.enabled() {
            nia_timing::emit_counter("llvm.units", self.outputs.len() as u64);
            nia_timing::emit_counter(
                "llvm.worker_lanes",
                worker_lanes.max(usize::from(!index.module_ids().is_empty())) as u64,
            );
            nia_timing::emit_counter("llvm.ready_task_submissions", self.partition_count as u64);
        }
        Ok(LlvmCodegenOutput {
            modules: self.outputs,
            diagnostics,
        })
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
    orchestration::emit_llvm_ir_with_options(lowering, type_store, session, options)
}

/// Validates backend IR and emits LTO pre-link modules.
///
/// This does not build a combined index or emit native objects. The returned
/// modules are an explicit intermediate product for a later whole-program
/// coordination step selected by `mode`.
pub fn emit_lto_modules(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
    pre_link: LtoPreLinkConfig,
    cache: Option<Arc<dyn LtoModuleWorkProductCache>>,
) -> LlvmLtoModuleOutput {
    orchestration::emit_lto_modules(lowering, type_store, session, options, pre_link, cache)
}

fn lto_linker_visible_symbols(index: &ProgramIndex) -> Vec<String> {
    let mut symbols = Vec::new();
    for module_id in index.module_ids() {
        let Some(module) = index.module(*module_id) else {
            continue;
        };
        symbols.extend(module.functions.iter().filter_map(|function| {
            match (&function.linkage, function.function_body.is_some()) {
                (nia_backend_ir::BackendLinkage::ExternExport { symbol }, true) => {
                    Some(symbol.clone())
                }
                _ => None,
            }
        }));
        symbols.extend(module.function_instances.iter().filter_map(|function| {
            match (&function.linkage, function.function_body.is_some()) {
                (nia_backend_ir::BackendLinkage::ExternExport { symbol }, true) => {
                    Some(symbol.clone())
                }
                _ => None,
            }
        }));
        symbols.extend(module.globals.iter().filter_map(|global| {
            match (&global.linkage, global.init.is_some()) {
                (nia_backend_ir::BackendLinkage::ExternExport { symbol }, true) => {
                    Some(symbol.clone())
                }
                _ => None,
            }
        }));
    }
    symbols.sort_unstable();
    symbols.dedup();
    symbols
}

/// Coordinates a complete set of summary modules and emits native ThinLTO objects.
///
/// This is a final-link operation: `input` must contain every source and
/// compiler-provided module in the linkage unit, and `preserved_symbols` must
/// name definitions observed by regular native linker inputs.
pub fn emit_thin_lto_objects(
    input: LlvmLtoModuleOutput,
    options: LlvmCodegenOptions,
    config: ThinLtoCodegenConfig<'_>,
) -> LlvmObjectOutput {
    if input.pre_link.mode != LtoMode::Thin {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                nia_span::Span::default(),
                "ThinLTO coordinator received full-LTO pre-link modules",
            )],
        };
    }
    if !input.diagnostics.is_empty() {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: input.diagnostics,
        };
    }
    let Some(target) = input.target else {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                nia_span::Span::default(),
                "ThinLTO module output is missing its target identity",
            )],
        };
    };
    let llvm_inputs = input
        .modules
        .iter()
        .map(|module| nia_llvm::lto::ThinLtoInput {
            name: &module.module_identifier,
            bitcode: &module.bitcode,
        })
        .collect::<Vec<_>>();
    let mut preserved_symbols = input.linker_visible_symbols;
    preserved_symbols.extend(
        config
            .preserved_symbols
            .iter()
            .map(|symbol| (*symbol).to_owned()),
    );
    preserved_symbols.sort_unstable();
    preserved_symbols.dedup();
    let preserved_symbol_refs = preserved_symbols
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let coordinated = match time_codegen_stage(options.timings, "llvm_thin_lto.coordinate", || {
        nia_llvm::lto::run_thin_lto(
            &llvm_inputs,
            nia_llvm::lto::ThinLtoConfig {
                target: &target,
                optimization: llvm_optimization_level(options.optimization.level),
                parallelism: config.parallelism,
                freestanding: input.pre_link.freestanding,
                preserved_symbols: &preserved_symbol_refs,
                backend_cache_directory: config.backend_cache_directory,
            },
        )
    }) {
        Ok(output) => output,
        Err(error) => {
            return LlvmObjectOutput {
                link_inputs: IncrementalLinkInputs::default(),
                diagnostics: vec![error.diagnostic()],
            };
        }
    };
    if let Some(error) = coordinated
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == nia_llvm::lto::ThinLtoDiagnosticSeverity::Error)
    {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![
                nia_diagnostic::Diagnostic::internal_error(
                    nia_diagnostic::codes::INTERNAL_LLVM_API,
                    format!("ThinLTO backend reported an error: {}", error.message),
                )
                .finish(),
            ],
        };
    }
    if options.timings.enabled() {
        nia_timing::emit_timing("llvm_thin_lto.thin_link", coordinated.timings.thin_link);
        nia_timing::emit_timing("llvm_thin_lto.backend", coordinated.timings.backend);
        nia_timing::emit_timing("llvm_thin_lto.promotion", coordinated.timings.promotion);
        nia_timing::emit_timing(
            "llvm_thin_lto.internalization",
            coordinated.timings.internalization,
        );
        nia_timing::emit_timing("llvm_thin_lto.import", coordinated.timings.import);
        nia_timing::emit_timing(
            "llvm_thin_lto.optimization",
            coordinated.timings.optimization,
        );
        nia_timing::emit_timing("llvm_thin_lto.codegen", coordinated.timings.codegen);
        nia_timing::emit_counter("llvm_thin_lto.modules", input.modules.len() as u64);
        nia_timing::emit_counter(
            "llvm_thin_lto.diagnostics",
            coordinated.diagnostics.len() as u64,
        );
        nia_timing::emit_counter("llvm_thin_lto.cache_hits", coordinated.cache.hits);
        nia_timing::emit_counter("llvm_thin_lto.cache_misses", coordinated.cache.misses);
        nia_timing::emit_counter("llvm_thin_lto.cache_corrupt", coordinated.cache.corrupt);
        nia_timing::emit_counter(
            "llvm_thin_lto.cache_read_errors",
            coordinated.cache.read_errors,
        );
        nia_timing::emit_counter(
            "llvm_thin_lto.cache_write_errors",
            coordinated.cache.write_errors,
        );
    }

    let mut modules = input
        .modules
        .into_iter()
        .map(|module| (module.module_identifier.clone(), module))
        .collect::<HashMap<_, _>>();
    let mut outputs = Vec::with_capacity(coordinated.objects.len());
    for object in coordinated.objects {
        let Some(module) = modules.remove(&object.module_name) else {
            return LlvmObjectOutput {
                link_inputs: IncrementalLinkInputs::default(),
                diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    nia_span::Span::default(),
                    format!(
                        "ThinLTO emitted an object for unknown module `{}`",
                        object.module_name
                    ),
                )],
            };
        };
        let fingerprint = thin_lto_backend_fingerprint(module.fingerprint, &object.cache_key);
        outputs.push(IncrementalLinkInput {
            key: module.key,
            fingerprint,
            object: NativeObject {
                unit: module.unit,
                name: module.name,
                bytes: object.bytes,
            },
        });
    }
    if !modules.is_empty() {
        let mut missing = modules.into_keys().collect::<Vec<_>>();
        missing.sort_unstable();
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                nia_span::Span::default(),
                format!(
                    "ThinLTO omitted backend objects for modules: {}",
                    missing.join(", ")
                ),
            )],
        };
    }
    outputs.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    match IncrementalLinkInputs::new(outputs) {
        Ok(link_inputs) => LlvmObjectOutput {
            link_inputs,
            diagnostics: Vec::new(),
        },
        Err(ice) => LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
        },
    }
}

fn thin_lto_backend_fingerprint(
    pre_link: CodegenUnitFingerprint,
    llvm_cache_key: &str,
) -> CodegenUnitFingerprint {
    let mut builder = QueryFingerprintBuilder::new(THIN_LTO_BACKEND_FINGERPRINT_DOMAIN);
    for part in pre_link.parts() {
        builder.write_u64(part);
    }
    builder.write_str(llvm_cache_key);
    CodegenUnitFingerprint::from_parts(builder.finish().parts())
}

/// Coordinates a complete set of regular-LTO modules and emits native partitions.
///
/// The returned objects belong to the final linkage unit rather than any source
/// codegen unit. Their stable keys are therefore owned by the logical entry
/// source supplied in `config` and qualified by LLVM's partition ordinal.
pub fn emit_full_lto_objects(
    input: LlvmLtoModuleOutput,
    options: LlvmCodegenOptions,
    config: FullLtoCodegenConfig<'_>,
) -> LlvmObjectOutput {
    if input.pre_link.mode != LtoMode::Full {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                nia_span::Span::default(),
                "full-LTO coordinator received ThinLTO pre-link modules",
            )],
        };
    }
    if !input.diagnostics.is_empty() {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: input.diagnostics,
        };
    }
    let Some(target) = input.target else {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                nia_span::Span::default(),
                "full-LTO module output is missing its target identity",
            )],
        };
    };
    let llvm_inputs = input
        .modules
        .iter()
        .map(|module| nia_llvm::lto::FullLtoInput {
            name: &module.module_identifier,
            bitcode: &module.bitcode,
        })
        .collect::<Vec<_>>();
    let mut preserved_symbols = input.linker_visible_symbols;
    preserved_symbols.extend(
        config
            .preserved_symbols
            .iter()
            .map(|symbol| (*symbol).to_owned()),
    );
    preserved_symbols.sort_unstable();
    preserved_symbols.dedup();
    let preserved_symbol_refs = preserved_symbols
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let coordinated = match time_codegen_stage(options.timings, "llvm_full_lto.coordinate", || {
        nia_llvm::lto::run_full_lto(
            &llvm_inputs,
            nia_llvm::lto::FullLtoConfig {
                target: &target,
                optimization: llvm_optimization_level(options.optimization.level),
                parallelism: config.parallelism,
                freestanding: input.pre_link.freestanding,
                preserved_symbols: &preserved_symbol_refs,
            },
        )
    }) {
        Ok(output) => output,
        Err(error) => {
            return LlvmObjectOutput {
                link_inputs: IncrementalLinkInputs::default(),
                diagnostics: vec![error.diagnostic()],
            };
        }
    };
    if let Some(error) = coordinated
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == nia_llvm::lto::ThinLtoDiagnosticSeverity::Error)
    {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![
                nia_diagnostic::Diagnostic::internal_error(
                    nia_diagnostic::codes::INTERNAL_LLVM_API,
                    format!("full-LTO backend reported an error: {}", error.message),
                )
                .finish(),
            ],
        };
    }
    if coordinated.objects.is_empty() {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::internal_error_at(
                nia_diagnostic::codes::INTERNAL_LLVM_API,
                nia_span::Span::default(),
                "full-LTO completed without emitting a native partition",
            )],
        };
    }
    if options.timings.enabled() {
        nia_timing::emit_timing("llvm_full_lto.total", coordinated.lto);
        nia_timing::emit_counter("llvm_full_lto.modules", input.modules.len() as u64);
        nia_timing::emit_counter("llvm_full_lto.partitions", coordinated.objects.len() as u64);
        nia_timing::emit_counter(
            "llvm_full_lto.diagnostics",
            coordinated.diagnostics.len() as u64,
        );
    }

    let mut outputs = Vec::with_capacity(coordinated.objects.len());
    for object in coordinated.objects {
        let key = CodegenUnitKey::LinkageUnit {
            source_identity: config.linkage_source_identity.clone(),
            ordinal: object.task,
        };
        outputs.push(IncrementalLinkInput {
            fingerprint: full_lto_backend_fingerprint(
                &input.modules,
                &target,
                options,
                config.parallelism,
                input.pre_link.freestanding,
                &preserved_symbol_refs,
                object.task,
            ),
            key,
            object: NativeObject {
                unit: CodegenUnitId::LinkageUnit {
                    ordinal: object.task,
                },
                name: format!("nia_full_lto_{}", object.task),
                bytes: object.bytes,
            },
        });
    }
    outputs.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    match IncrementalLinkInputs::new(outputs) {
        Ok(link_inputs) => LlvmObjectOutput {
            link_inputs,
            diagnostics: Vec::new(),
        },
        Err(ice) => LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
        },
    }
}

fn full_lto_backend_fingerprint(
    modules: &[LtoModule],
    target: &TargetMachineIdentity,
    options: LlvmCodegenOptions,
    parallelism: usize,
    freestanding: bool,
    preserved_symbols: &[&str],
    ordinal: u32,
) -> CodegenUnitFingerprint {
    let mut builder = QueryFingerprintBuilder::new(FULL_LTO_BACKEND_FINGERPRINT_DOMAIN);
    builder.write_str("regular-lto:monolithic-ipo-and-partitioned-codegen");
    builder.write_u64(modules.len() as u64);
    for module in modules {
        builder.write_str(&module.module_identifier);
        for part in module.fingerprint.parts() {
            builder.write_u64(part);
        }
    }
    builder.write_str(&target.triple);
    builder.write_str(&target.cpu);
    builder.write_str(&target.features);
    fingerprint::write_toolchain_identity(&mut builder, options.toolchain_identity);
    fingerprint::write_optimization(&mut builder, options.optimization);
    builder.write_u64(parallelism as u64);
    builder.write_u8(u8::from(freestanding));
    builder.write_u64(preserved_symbols.len() as u64);
    for symbol in preserved_symbols {
        builder.write_str(symbol);
    }
    builder.write_u64(u64::from(ordinal));
    CodegenUnitFingerprint::from_parts(builder.finish().parts())
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
    orchestration::emit_native_objects(lowering, type_store, session, options, cache)
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

enum NativeCodegenTask {
    Partition(Box<CodegenPartitionPreparation>),
    DeclarationModule(ModuleId),
    CompilerBuiltins(compiler_builtins::CompilerBuiltinSymbols),
}

enum LtoCodegenTask {
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
) -> nia_ice::IceResult<(Arc<ProgramIndex>, Vec<CodegenPartitionPreparation>)> {
    let module_ids = modules.module_ids().to_vec();
    let mut coordinator = CodegenReadinessCoordinator::new(modules, type_store, owners);
    let mut preparations = Vec::new();
    for module_id in module_ids {
        preparations.extend(coordinator.publish(module_id)?);
    }
    preparations.sort_unstable_by(|left, right| left.key().cmp(right.key()));
    Ok((coordinator.finish()?, preparations))
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

fn emit_llvm_ir_partition(
    prepared: PreparedCodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
) -> Result<LlvmModuleOutput, Vec<nia_diagnostic::Diagnostic>> {
    let PreparedCodegenPartition {
        partition,
        declarations,
    } = prepared;
    let Some(module) = index.module_for_partition(&partition) else {
        return Err(vec![nia_diagnostic::Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            "codegen partition has no matching published owner module",
        )]);
    };
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
    let memory_permit = nia_query::acquire_llvm_memory_permit()
        .map_err(|ice| vec![nia_diagnostic::Diagnostic::from(ice)])?;
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

fn emit_lto_partition(
    prepared: PreparedCodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
    pre_link: LtoPreLinkConfig,
    target_identity: &TargetMachineIdentity,
    cache: Option<&dyn LtoModuleWorkProductCache>,
) -> Result<(LtoModule, WorkProductReuse), Vec<nia_diagnostic::Diagnostic>> {
    let PreparedCodegenPartition {
        partition,
        declarations,
    } = prepared;
    let Some(module) = index.module_for_partition(&partition) else {
        return Err(vec![nia_diagnostic::Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            "codegen partition has no matching published owner module",
        )]);
    };
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
        fingerprint::ArtifactTarget::LtoBitcode(pre_link, target_identity),
    )?;
    let module_identifier = lto_module_identifier(&partition.key);
    let miss = match load_lto_work_product(cache, pre_link.mode, &partition.key, fingerprints) {
        WorkProductReuseLookup::Hit(bitcode) => {
            return Ok((
                LtoModule {
                    unit: partition.id,
                    key: partition.key,
                    fingerprint: fingerprints.fingerprint,
                    name: module.name.clone(),
                    module_identifier,
                    bitcode,
                },
                WorkProductReuse::Hit,
            ));
        }
        WorkProductReuseLookup::Miss(miss) => miss,
    };
    let memory_permit = nia_query::acquire_llvm_memory_permit()
        .map_err(|ice| vec![nia_diagnostic::Diagnostic::from(ice)])?;
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            target_identity,
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
    let bitcode = codegen
        .emit_lto_bitcode(
            &target,
            &module_identifier,
            llvm_optimization_level(options.optimization.level),
            pre_link,
        )
        .map_err(|diagnostic| vec![diagnostic])?;
    let write_error =
        publish_lto_work_product(cache, pre_link.mode, &partition.key, fingerprints, &bitcode);
    Ok((
        LtoModule {
            unit: partition.id,
            key: partition.key,
            fingerprint: fingerprints.fingerprint,
            name: module.name.clone(),
            module_identifier,
            bitcode,
        },
        WorkProductReuse::Miss {
            reason: miss,
            write_error,
        },
    ))
}

fn lto_module_identifier(key: &CodegenUnitKey) -> String {
    match key {
        CodegenUnitKey::SourceModule {
            source_identity,
            ordinal,
        } => {
            let path = source_identity.normalized_path();
            format!("nia:cgu:{}:{path}:{ordinal}", path.len())
        }
        CodegenUnitKey::CompilerBuiltins => "nia:cgu:compiler-builtins".to_owned(),
        CodegenUnitKey::LinkageUnit {
            source_identity,
            ordinal,
        } => {
            let path = source_identity.normalized_path();
            format!("nia:linkage-unit:{}:{path}:{ordinal}", path.len())
        }
    }
}

fn emit_native_object_partition(
    prepared: PreparedCodegenPartition,
    index: Arc<ProgramIndex>,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(IncrementalLinkInput<NativeObject>, WorkProductReuse), Vec<nia_diagnostic::Diagnostic>>
{
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
    let Some(module) = index.module_for_partition(&partition) else {
        return Err(vec![nia_diagnostic::Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            "codegen partition has no matching published owner module",
        )]);
    };
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
    let miss = match load_object_work_product(cache, &partition.key, fingerprints) {
        WorkProductReuseLookup::Hit(bytes) => {
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
                WorkProductReuse::Hit,
            ));
        }
        WorkProductReuseLookup::Miss(miss) => miss,
    };
    let memory_permit = nia_query::acquire_llvm_memory_permit()
        .map_err(|ice| vec![nia_diagnostic::Diagnostic::from(ice)])?;
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
    let write_error = publish_object_work_product(cache, &partition.key, fingerprints, &bytes);
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
        WorkProductReuse::Miss {
            reason: miss,
            write_error,
        },
    ))
}

fn emit_compiler_builtins_lto_module(
    symbols: compiler_builtins::CompilerBuiltinSymbols,
    options: LlvmCodegenOptions,
    pre_link: LtoPreLinkConfig,
    target_identity: &TargetMachineIdentity,
    cache: Option<&dyn LtoModuleWorkProductCache>,
) -> Result<(LtoModule, WorkProductReuse), nia_diagnostic::Diagnostic> {
    let fingerprints = fingerprint::compiler_builtins_lto_fingerprint(
        &symbols,
        options,
        target_identity,
        pre_link,
    );
    let miss = match load_lto_work_product(
        cache,
        pre_link.mode,
        &CodegenUnitKey::CompilerBuiltins,
        fingerprints,
    ) {
        WorkProductReuseLookup::Hit(bitcode) => {
            return Ok((
                LtoModule {
                    unit: CodegenUnitId::CompilerBuiltins,
                    key: CodegenUnitKey::CompilerBuiltins,
                    fingerprint: fingerprints.fingerprint,
                    name: "nia.compiler_builtins".to_owned(),
                    module_identifier: "nia:cgu:compiler-builtins".to_owned(),
                    bitcode,
                },
                WorkProductReuse::Hit,
            ));
        }
        WorkProductReuseLookup::Miss(miss) => miss,
    };
    let memory_permit =
        nia_query::acquire_llvm_memory_permit().map_err(nia_diagnostic::Diagnostic::from)?;
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            target_identity,
            llvm_optimization_level(options.optimization.level),
        )
    })
    .map_err(|error| error.diagnostic())?;
    let bitcode = compiler_builtins::emit_lto_bitcode(
        &target,
        symbols,
        llvm_optimization_level(options.optimization.level),
        pre_link,
    )?;
    let write_error = publish_lto_work_product(
        cache,
        pre_link.mode,
        &CodegenUnitKey::CompilerBuiltins,
        fingerprints,
        &bitcode,
    );
    Ok((
        LtoModule {
            unit: CodegenUnitId::CompilerBuiltins,
            key: CodegenUnitKey::CompilerBuiltins,
            fingerprint: fingerprints.fingerprint,
            name: "nia.compiler_builtins".to_owned(),
            module_identifier: "nia:cgu:compiler-builtins".to_owned(),
            bitcode,
        },
        WorkProductReuse::Miss {
            reason: miss,
            write_error,
        },
    ))
}

fn emit_compiler_builtins_object(
    symbols: compiler_builtins::CompilerBuiltinSymbols,
    options: LlvmCodegenOptions,
    cache: Option<&dyn ObjectWorkProductCache>,
) -> Result<(IncrementalLinkInput<NativeObject>, WorkProductReuse), nia_diagnostic::Diagnostic> {
    let target_identity = time_codegen_stage(
        options.timings,
        "llvm_codegen.native_target_identity",
        TargetMachine::native_identity,
    )
    .map_err(|error| error.diagnostic())?;
    let fingerprints =
        fingerprint::compiler_builtins_fingerprint(&symbols, options, &target_identity);
    let miss =
        match load_object_work_product(cache, &CodegenUnitKey::CompilerBuiltins, fingerprints) {
            WorkProductReuseLookup::Hit(bytes) => {
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
                    WorkProductReuse::Hit,
                ));
            }
            WorkProductReuseLookup::Miss(miss) => miss,
        };
    let memory_permit =
        nia_query::acquire_llvm_memory_permit().map_err(nia_diagnostic::Diagnostic::from)?;
    record_memory_permit(options.timings, memory_permit.waited());
    let target = time_codegen_stage(options.timings, "llvm_codegen.native_target", || {
        TargetMachine::for_identity(
            &target_identity,
            llvm_optimization_level(options.optimization.level),
        )
    })
    .map_err(|error| error.diagnostic())?;
    let bytes = compiler_builtins::emit_object(&target, symbols)?;
    let write_error = publish_object_work_product(
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
        WorkProductReuse::Miss {
            reason: miss,
            write_error,
        },
    ))
}

enum WorkProductReuseLookup {
    Hit(Vec<u8>),
    Miss(WorkProductReuseMiss),
}

fn load_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
) -> WorkProductReuseLookup {
    let Some(cache) = cache else {
        return WorkProductReuseLookup::Miss(WorkProductReuseMiss::Disabled);
    };
    match cache.load(key, fingerprints) {
        Ok(CodegenWorkProductLookup::Hit(bytes)) => WorkProductReuseLookup::Hit(bytes),
        Ok(CodegenWorkProductLookup::NotFound) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::NotFound)
        }
        Ok(CodegenWorkProductLookup::Invalidated(reasons)) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::Invalidated(reasons))
        }
        Ok(CodegenWorkProductLookup::Corrupt) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::Corrupt)
        }
        Err(_) => WorkProductReuseLookup::Miss(WorkProductReuseMiss::ReadError),
    }
}

fn load_lto_work_product(
    cache: Option<&dyn LtoModuleWorkProductCache>,
    mode: LtoMode,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
) -> WorkProductReuseLookup {
    let Some(cache) = cache else {
        return WorkProductReuseLookup::Miss(WorkProductReuseMiss::Disabled);
    };
    match cache.load(mode, key, fingerprints) {
        Ok(CodegenWorkProductLookup::Hit(bytes)) => WorkProductReuseLookup::Hit(bytes),
        Ok(CodegenWorkProductLookup::NotFound) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::NotFound)
        }
        Ok(CodegenWorkProductLookup::Invalidated(reasons)) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::Invalidated(reasons))
        }
        Ok(CodegenWorkProductLookup::Corrupt) => {
            WorkProductReuseLookup::Miss(WorkProductReuseMiss::Corrupt)
        }
        Err(_) => WorkProductReuseLookup::Miss(WorkProductReuseMiss::ReadError),
    }
}

fn publish_object_work_product(
    cache: Option<&dyn ObjectWorkProductCache>,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
) -> bool {
    cache.is_some_and(|cache| cache.publish(key, fingerprints, bytes).is_err())
}

fn publish_lto_work_product(
    cache: Option<&dyn LtoModuleWorkProductCache>,
    mode: LtoMode,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
) -> bool {
    cache.is_some_and(|cache| cache.publish(mode, key, fingerprints, bytes).is_err())
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
