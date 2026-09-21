// SPDX-License-Identifier: GPL-3.0-or-later
//! Whole-program LLVM IR orchestration.

use super::*;

pub(super) fn emit_llvm_ir_with_options(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    let timings = options.timings;
    if let Err(ice) = lowering
        .codegen_partitions
        .validate_program(&lowering.program)
    {
        return LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
        };
    }
    let module_store = lowering.program.module_store();
    let owners = Arc::clone(&lowering.owner_directory);
    let (index, preparations) =
        match time_codegen_stage(timings, "llvm_codegen.program_index", || {
            prepare_complete_codegen(module_store, type_store, owners)
        }) {
            Ok(value) => value,
            Err(ice) => {
                return LlvmCodegenOutput {
                    modules: Vec::new(),
                    diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
                };
            }
        };
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
    let outcomes = match session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            move || {
                Ok(match task {
                    LlvmIrTask::Partition(preparation) => match *preparation {
                        CodegenPartitionPreparation::Ready(prepared) => {
                            emit_llvm_ir_partition(prepared, index, options).map(Some)
                        }
                        CodegenPartitionPreparation::Invalid { diagnostics, .. } => {
                            Err(diagnostics)
                        }
                    },
                    LlvmIrTask::DeclarationModule(module_id) => {
                        validate_declaration_module(module_id, &index).map(|()| None)
                    }
                })
            }
        }),
        nia_query::llvm_memory_task_capacity(),
    ) {
        Ok(outcomes) => outcomes,
        Err(ice) => {
            return LlvmCodegenOutput {
                modules: Vec::new(),
                diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
            };
        }
    };
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

pub(super) fn emit_native_objects(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
    cache: Option<Arc<dyn ObjectWorkProductCache>>,
) -> LlvmObjectOutput {
    let timings = options.timings;
    if let Err(ice) = lowering
        .codegen_partitions
        .validate_program(&lowering.program)
    {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
            diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
        };
    }
    let module_store = lowering.program.module_store();
    let owners = Arc::clone(&lowering.owner_directory);
    let (index, preparations) =
        match time_codegen_stage(timings, "llvm_codegen.program_index", || {
            prepare_complete_codegen(module_store, type_store, owners)
        }) {
            Ok(value) => value,
            Err(ice) => {
                return LlvmObjectOutput {
                    link_inputs: IncrementalLinkInputs::default(),
                    diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
                };
            }
        };
    let builtin_symbols = compiler_builtins::required_symbols(&index);
    let program_diagnostics = validate_native_backend_program(&index, builtin_symbols);
    if !program_diagnostics.is_empty() {
        return LlvmObjectOutput {
            link_inputs: IncrementalLinkInputs::default(),
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
    let outcomes = match session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            let cache = cache.clone();
            move || {
                Ok(match task {
                    NativeCodegenTask::Partition(preparation) => match *preparation {
                        CodegenPartitionPreparation::Ready(prepared) => {
                            emit_native_object_partition(prepared, index, options, cache.as_deref())
                                .map(Some)
                        }
                        CodegenPartitionPreparation::Invalid { diagnostics, .. } => {
                            Err(diagnostics)
                        }
                    },
                    NativeCodegenTask::DeclarationModule(module_id) => {
                        validate_declaration_module(module_id, &index).map(|()| None)
                    }
                    NativeCodegenTask::CompilerBuiltins(symbols) => {
                        emit_compiler_builtins_object(symbols, options, cache.as_deref())
                            .map(Some)
                            .map_err(|diagnostic| vec![diagnostic])
                    }
                })
            }
        }),
        nia_query::llvm_memory_task_capacity(),
    ) {
        Ok(outcomes) => outcomes,
        Err(ice) => {
            return LlvmObjectOutput {
                link_inputs: IncrementalLinkInputs::default(),
                diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
            };
        }
    };
    let mut outputs = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    let mut reuse_counts = WorkProductReuseCounts::default();
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
        reuse_counts.emit("object");
    }
    match IncrementalLinkInputs::new(outputs) {
        Ok(link_inputs) => LlvmObjectOutput {
            link_inputs,
            diagnostics,
        },
        Err(ice) => {
            diagnostics.push(nia_diagnostic::Diagnostic::from(ice));
            LlvmObjectOutput {
                link_inputs: IncrementalLinkInputs::default(),
                diagnostics,
            }
        }
    }
}

pub(super) fn emit_lto_modules(
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    session: &QuerySession,
    options: LlvmCodegenOptions,
    pre_link: LtoPreLinkConfig,
    cache: Option<Arc<dyn LtoModuleWorkProductCache>>,
) -> LlvmLtoModuleOutput {
    let timings = options.timings;
    if let Err(ice) = lowering
        .codegen_partitions
        .validate_program(&lowering.program)
    {
        return LlvmLtoModuleOutput {
            pre_link,
            target: None,
            modules: Vec::new(),
            linker_visible_symbols: Vec::new(),
            diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
        };
    }
    let module_store = lowering.program.module_store();
    let owners = Arc::clone(&lowering.owner_directory);
    let (index, preparations) =
        match time_codegen_stage(timings, "llvm_codegen.program_index", || {
            prepare_complete_codegen(module_store, type_store, owners)
        }) {
            Ok(value) => value,
            Err(ice) => {
                return LlvmLtoModuleOutput {
                    pre_link,
                    target: None,
                    modules: Vec::new(),
                    linker_visible_symbols: Vec::new(),
                    diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
                };
            }
        };
    let builtin_symbols = compiler_builtins::required_symbols(&index);
    let program_diagnostics = validate_native_backend_program(&index, builtin_symbols);
    if !program_diagnostics.is_empty() {
        return LlvmLtoModuleOutput {
            pre_link,
            target: None,
            modules: Vec::new(),
            linker_visible_symbols: Vec::new(),
            diagnostics: program_diagnostics,
        };
    }
    let target_identity =
        match time_codegen_stage(timings, "llvm_codegen.native_target_identity", || {
            TargetMachine::native_identity()
        }) {
            Ok(identity) => Arc::new(identity),
            Err(error) => {
                return LlvmLtoModuleOutput {
                    pre_link,
                    target: None,
                    modules: Vec::new(),
                    linker_visible_symbols: Vec::new(),
                    diagnostics: vec![error.diagnostic()],
                };
            }
        };
    let has_partitions = !preparations.is_empty();
    let mut tasks = preparations
        .into_iter()
        .map(|preparation| LtoCodegenTask::Partition(Box::new(preparation)))
        .collect::<Vec<_>>();
    tasks.extend(
        declaration_only_modules(&index, has_partitions)
            .into_iter()
            .map(LtoCodegenTask::DeclarationModule),
    );
    if builtin_symbols.any() {
        tasks.push(LtoCodegenTask::CompilerBuiltins(builtin_symbols));
    }
    let worker_lanes = codegen_worker_lanes(session, tasks.len());
    let outcomes = match session.run_tasks_bounded(
        tasks.into_iter().map(|task| {
            let index = Arc::clone(&index);
            let target_identity = Arc::clone(&target_identity);
            let cache = cache.clone();
            move || {
                Ok(match task {
                    LtoCodegenTask::Partition(preparation) => match *preparation {
                        CodegenPartitionPreparation::Ready(prepared) => emit_lto_partition(
                            prepared,
                            index,
                            options,
                            pre_link,
                            &target_identity,
                            cache.as_deref(),
                        )
                        .map(Some),
                        CodegenPartitionPreparation::Invalid { diagnostics, .. } => {
                            Err(diagnostics)
                        }
                    },
                    LtoCodegenTask::DeclarationModule(module_id) => {
                        validate_declaration_module(module_id, &index).map(|()| None)
                    }
                    LtoCodegenTask::CompilerBuiltins(symbols) => emit_compiler_builtins_lto_module(
                        symbols,
                        options,
                        pre_link,
                        &target_identity,
                        cache.as_deref(),
                    )
                    .map(Some)
                    .map_err(|diagnostic| vec![diagnostic]),
                })
            }
        }),
        nia_query::llvm_memory_task_capacity(),
    ) {
        Ok(outcomes) => outcomes,
        Err(ice) => {
            return LlvmLtoModuleOutput {
                pre_link,
                target: Some(Arc::unwrap_or_clone(target_identity)),
                modules: Vec::new(),
                linker_visible_symbols: lto_linker_visible_symbols(&index),
                diagnostics: vec![nia_diagnostic::Diagnostic::from(ice)],
            };
        }
    };
    let mut modules = Vec::with_capacity(outcomes.len());
    let mut diagnostics = Vec::new();
    let mut reuse_counts = WorkProductReuseCounts::default();
    for outcome in outcomes {
        match outcome {
            Ok(Some((module, reuse))) => {
                reuse_counts.record(reuse);
                modules.push(module);
            }
            Ok(None) => {}
            Err(task_diagnostics) => diagnostics.extend(task_diagnostics),
        }
    }
    modules.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    if timings.enabled() {
        nia_timing::emit_counter("llvm.units", modules.len() as u64);
        nia_timing::emit_counter("llvm.worker_lanes", worker_lanes as u64);
        reuse_counts.emit("lto_prelink");
    }
    LlvmLtoModuleOutput {
        pre_link,
        target: Some(Arc::unwrap_or_clone(target_identity)),
        modules,
        linker_visible_symbols: lto_linker_visible_symbols(&index),
        diagnostics,
    }
}
