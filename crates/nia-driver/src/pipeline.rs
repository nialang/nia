// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use nia_compiler_query::{
    BackendFunctionStats, CodegenScope, CompileRequest, CompilerDatabase, TimingMode,
    has_error_diagnostics, query_error_diagnostic,
};
use nia_diagnostic::Diagnostic;
use nia_loader_query::{LoadRequest, LoaderDatabase, SourceInputManifest};
use nia_opt::OptimizationPolicy;
use nia_source::{SourceDatabase, SourceIdentity, SourcePath};
use nia_toolchain::ToolchainLayout;

use crate::{CheckedProgram, CodegenProgram, ProgramDiagnostic};

mod artifact_cache;
mod compilation;
mod contracts;
mod counters;
mod linking;
mod output;
mod requests;
mod sessions;

pub use contracts::{
    CheckedProgramWithSourceManifest, DriverConfig, ExecutableCacheEnvironment,
    ExecutableCacheReference, ExecutableCacheRestore, LinkedExecutableWithSourceManifest,
    StaticArchiveCacheEnvironment, StaticArchiveCacheReference, StaticArchiveCacheRestore,
};
pub use requests::{
    CheckRequest, EmitLlvmRequest, EmitObjectRequest, LinkExecutableRequest, LinkTimeOptimization,
    ObjectOutput, WriteObjectRequest,
};

use counters::*;
use output::{object_file_name, write_output_file};
use sessions::{LoaderKey, SessionCompiler, SessionLoader};

/// Stateful compiler driver reusing source, loader, and compiler sessions.
#[derive(Debug)]
pub struct Driver {
    config: DriverConfig,
    sources: SourceDatabase,
    loader: std::sync::Arc<Mutex<Option<SessionLoader>>>,
    compiler: std::sync::Arc<Mutex<Option<SessionCompiler>>>,
    object_cache: Option<std::sync::Arc<crate::object_cache::PersistentObjectWorkProductCache>>,
    lto_module_cache:
        Option<std::sync::Arc<crate::object_cache::PersistentLtoModuleWorkProductCache>>,
    thin_lto_backend_cache_directory: Option<PathBuf>,
    link_cache: Option<std::sync::Arc<crate::executable_cache::PersistentLinkResultCache>>,
    archive_cache: Option<std::sync::Arc<crate::archive_cache::PersistentArchiveCache>>,
    #[cfg(test)]
    _test_resources: Option<nia_test_support::TestResourceSession<'static>>,
}

impl Driver {
    /// Creates a driver with default configuration for `toolchain`.
    pub fn new(toolchain: std::sync::Arc<ToolchainLayout>) -> Self {
        Self::with_config(DriverConfig::new(toolchain))
    }

    /// Creates a driver with explicit configuration and optional caches.
    pub fn with_config(config: DriverConfig) -> Self {
        let object_cache = config.artifact_cache_dir.as_ref().map(|path| {
            std::sync::Arc::new(crate::object_cache::PersistentObjectWorkProductCache::new(
                path.clone(),
            ))
        });
        let lto_module_cache = config.artifact_cache_dir.as_ref().map(|path| {
            std::sync::Arc::new(
                crate::object_cache::PersistentLtoModuleWorkProductCache::new(path.clone()),
            )
        });
        let thin_lto_backend_cache_directory = config.artifact_cache_dir.as_ref().map(|path| {
            let [first, second] = nia_toolchain::ToolchainIdentityFingerprint::current().parts();
            path.join("artifacts")
                .join("thin-lto-backends")
                .join(nia_compat::formats::THIN_LTO_BACKEND_CACHE.path_component)
                .join(format!("{first:016x}{second:016x}"))
        });
        let link_cache = config.artifact_cache_dir.as_ref().map(|path| {
            std::sync::Arc::new(crate::executable_cache::PersistentLinkResultCache::new(
                path.clone(),
            ))
        });
        let archive_cache = config.artifact_cache_dir.as_ref().map(|path| {
            std::sync::Arc::new(crate::archive_cache::PersistentArchiveCache::new(
                path.clone(),
            ))
        });
        Self {
            config,
            sources: SourceDatabase::new(),
            loader: std::sync::Arc::new(Mutex::new(None)),
            compiler: std::sync::Arc::new(Mutex::new(None)),
            object_cache,
            lto_module_cache,
            thin_lto_backend_cache_directory,
            link_cache,
            archive_cache,
            #[cfg(test)]
            _test_resources: nia_test_support::acquire_test_resources_if_needed(
                nia_test_support::TestWorkload::Compiler,
            ),
        }
    }

    /// Returns the immutable driver configuration.
    pub fn config(&self) -> &DriverConfig {
        &self.config
    }

    /// Returns the source database owned by this driver.
    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    #[cfg(test)]
    pub(crate) fn compiler_query_executions(&self, name: &str) -> usize {
        self.compiler
            .lock()
            .expect("driver compiler lock poisoned")
            .as_ref()
            .map(|compiler| {
                compiler
                    .database
                    .query_trace()
                    .expect("test compiler query trace")
                    .queries
                    .iter()
                    .filter(|query| query.frame.name == name)
                    .map(|query| query.stats.executions)
                    .sum()
            })
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn compiler_provider_demand_rounds(&self) -> u64 {
        self.compiler
            .lock()
            .expect("driver compiler lock poisoned")
            .as_ref()
            .map(|compiler| compiler.database.provider_demand_rounds())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn loader_and_compiler_share_query_session(&self) -> bool {
        let loader = self.loader.lock().expect("driver loader lock poisoned");
        let compiler = self.compiler.lock().expect("driver compiler lock poisoned");
        match (loader.as_ref(), compiler.as_ref()) {
            (Some(loader), Some(compiler)) => loader
                .database
                .query_session()
                .ptr_eq(&compiler.database.query_session()),
            _ => false,
        }
    }

    /// Emits LLVM IR modules for a checked request.
    pub fn emit_llvm_ir(&self, request: EmitLlvmRequest) -> DriverOutput<LlvmIrArtifact> {
        {
            let timings = request.check.timings;
            let database = match self.compiler_database(&request.check) {
                Ok(database) => database,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            let preparation = match database.codegen_preparation() {
                Ok(preparation) => preparation,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if has_error_diagnostics(&preparation.diagnostics) {
                return DriverOutput::from_error(DriverError::CodegenPreparationDiagnostics(
                    preparation.diagnostics,
                ));
            }
            let checked_body_count = preparation
                .modules
                .iter()
                .map(|module| module.body_ir.function_bodies.len())
                .sum();
            let checked_module_count = preparation.modules.len();
            let monomorphized_instance_count = preparation.monomorphization.instances.len();
            let options = codegen_options(
                preparation.optimization,
                timings,
                self.config.toolchain.identity().fingerprint(),
            );
            let optimization = preparation.optimization;
            let diagnostics = preparation.diagnostics;
            let type_store = std::sync::Arc::clone(&preparation.type_store);
            let session = database.query_session();
            let result = database.with_backend_finalization_schedule(|schedule| {
                Ok((|| -> Result<_, DriverError> {
                    match schedule {
                        Err(lowering) => Err(DriverError::CodegenDiagnostics(lowering.diagnostics)),
                        Ok(mut schedule) => {
                            let mut emitter = nia_codegen_llvm::LlvmIrReadinessEmitter::new(
                                schedule.module_store(),
                                type_store,
                                schedule.owner_directory(),
                                options,
                                &session,
                            )
                            .map_err(|error| {
                                DriverError::InternalDiagnostic(Diagnostic::from(error))
                            })?;
                            while let Some(ready) = schedule.wait_next().map_err(|error| {
                                DriverError::InternalDiagnostic(query_error_diagnostic(error))
                            })? {
                                emitter.publish(ready).map_err(|error| {
                                    DriverError::InternalDiagnostic(Diagnostic::from(error))
                                })?;
                            }
                            let lowering = schedule.finish().map_err(|error| {
                                DriverError::InternalDiagnostic(query_error_diagnostic(error))
                            })?;
                            if !lowering.diagnostics.is_empty() {
                                return Err(DriverError::CodegenDiagnostics(lowering.diagnostics));
                            }
                            let backend_function_stats = lowering.program.function_stats();
                            let backend_module_count = lowering.program.modules.len();
                            Ok((
                                emitter.finish().map_err(|error| {
                                    DriverError::InternalDiagnostic(Diagnostic::from(error))
                                })?,
                                backend_function_stats,
                                backend_module_count,
                                lowering.optimization_report,
                            ))
                        }
                    }
                })())
            });
            let (output, backend_function_stats, backend_module_count, optimization_report) =
                match result {
                    Ok(Ok(output)) => output,
                    Ok(Err(error)) => return DriverOutput::from_error(error),
                    Err(error) => {
                        return DriverOutput::from_error(DriverError::InternalDiagnostic(
                            query_error_diagnostic(error),
                        ));
                    }
                };
            let loader_trace = match self.loader_query_trace() {
                Ok(trace) => trace,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if let Err(error) = emit_compilation_counters(
                timings,
                &database,
                &loader_trace,
                &LiveCodegenCounters {
                    checked_body_count,
                    reachable_body_count: backend_function_stats.definitions(),
                    backend_function_stats,
                    link_input_count: None,
                    checked_module_count,
                    monomorphized_instance_count,
                    backend_module_count,
                },
                database.provider_demand_rounds(),
                self.sources.source_table_stats(),
            ) {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            DriverOutput::success(LlvmIrArtifact {
                modules: output.modules,
                optimization,
                optimization_report,
                diagnostics,
            })
        }
    }

    /// Emits LLVM IR from an already checked codegen product.
    pub fn emit_llvm_ir_from_codegen(
        &self,
        program: &CodegenProgram,
    ) -> DriverOutput<LlvmIrArtifact> {
        self.emit_llvm_ir_from_codegen_with_timings(program, TimingMode::Off)
    }

    /// Emits LLVM IR from codegen while overriding timing mode.
    pub fn emit_llvm_ir_from_codegen_with_timings(
        &self,
        program: &CodegenProgram,
        timings: TimingMode,
    ) -> DriverOutput<LlvmIrArtifact> {
        {
            let session = match self.codegen_query_session() {
                Ok(session) => session,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            let output = nia_codegen_llvm::emit_llvm_ir_with_options(
                std::sync::Arc::clone(&program.backend_lowering),
                std::sync::Arc::clone(&program.type_store),
                &session,
                codegen_options(
                    program.optimization,
                    timings,
                    self.config.toolchain.identity().fingerprint(),
                ),
            );
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            DriverOutput::success(LlvmIrArtifact {
                modules: output.modules,
                optimization: program.optimization,
                optimization_report: program.backend_lowering.optimization_report.clone(),
                diagnostics: program.diagnostics.clone(),
            })
        }
    }

    /// Emits native object bytes for a checked request.
    pub fn emit_native_objects(&self, request: EmitObjectRequest) -> DriverOutput<ObjectArtifact> {
        self.emit_objects_with_source_manifest(request, ObjectEmissionMode::NoLto)
            .map(|output| output.artifact)
    }

    fn emit_objects_with_source_manifest(
        &self,
        request: EmitObjectRequest,
        mode: ObjectEmissionMode<'_>,
    ) -> DriverOutput<ObjectArtifactWithSourceManifest> {
        {
            let timings = request.check.timings;
            let (database, loader) =
                match time_detail_stage(timings, mode.prepare_database_stage(), || {
                    self.compilation_databases_with_codegen_scope(
                        &request.check,
                        CodegenScope::Entry,
                    )
                }) {
                    Ok(databases) => databases,
                    Err(error) => {
                        return DriverOutput::from_error(DriverError::InternalDiagnostic(
                            query_error_diagnostic(error),
                        ));
                    }
                };
            let emission = match time_detail_stage(timings, mode.emit_objects_stage(), || {
                self.emit_objects_for_database(&database, timings, mode)
            }) {
                Ok(emission) => emission,
                Err(error) => return DriverOutput::from_error(error),
            };
            let loader_trace = match time_detail_stage(timings, mode.loader_trace_stage(), || {
                self.loader_query_trace()
            }) {
                Ok(trace) => trace,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if let Err(error) = time_detail_stage(timings, mode.emit_counters_stage(), || {
                emit_compilation_counters(
                    timings,
                    &database,
                    &loader_trace,
                    &LiveCodegenCounters {
                        checked_body_count: emission.checked_body_count,
                        reachable_body_count: emission.reachable_body_count,
                        backend_function_stats: emission.backend_function_stats,
                        link_input_count: Some(emission.link_input_count),
                        checked_module_count: emission.checked_module_count,
                        monomorphized_instance_count: emission.monomorphized_instance_count,
                        backend_module_count: emission.backend_module_count,
                    },
                    database.provider_demand_rounds(),
                    self.sources.source_table_stats(),
                )
            }) {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
            let source_manifest =
                match time_detail_stage(timings, mode.source_manifest_stage(), || {
                    loader.source_input_manifest()
                }) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        return DriverOutput::from_error(DriverError::InternalDiagnostic(
                            query_error_diagnostic(error),
                        ));
                    }
                };
            DriverOutput::success(ObjectArtifactWithSourceManifest {
                artifact: emission.artifact,
                source_manifest,
            })
        }
    }

    fn emit_objects_for_database(
        &self,
        database: &CompilerDatabase,
        timings: TimingMode,
        mode: ObjectEmissionMode<'_>,
    ) -> Result<NativeDatabaseEmission, DriverError> {
        let preparation = time_detail_stage(timings, mode.codegen_preparation_stage(), || {
            database.codegen_preparation()
        })
        .map_err(|error| DriverError::InternalDiagnostic(query_error_diagnostic(error)))?;
        if has_error_diagnostics(&preparation.diagnostics) {
            return Err(DriverError::CodegenPreparationDiagnostics(
                preparation.diagnostics,
            ));
        }
        let checked_body_count = preparation
            .modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum();
        let checked_module_count = preparation.modules.len();
        let monomorphized_instance_count = preparation.monomorphization.instances.len();
        let optimization = preparation.optimization;
        let diagnostics = preparation.diagnostics;
        let type_store = std::sync::Arc::clone(&preparation.type_store);
        let options = codegen_options(
            optimization,
            timings,
            self.config.toolchain.identity().fingerprint(),
        );
        let session = database.query_session();
        let object_cache = self.object_cache.as_ref().map(|cache| {
            cache.clone() as std::sync::Arc<dyn nia_codegen_llvm::ObjectWorkProductCache>
        });
        let lto_module_cache = self.lto_module_cache.as_ref().map(|cache| {
            cache.clone() as std::sync::Arc<dyn nia_codegen_llvm::LtoModuleWorkProductCache>
        });
        let lto_parallelism = session
            .executor_parallelism()
            .min(nia_query::llvm_memory_task_capacity())
            .max(1);
        let (output, backend_function_stats, backend_module_count, optimization_report) = database
            .with_backend_finalization_schedule(|schedule| {
                Ok((|| -> Result<_, DriverError> {
                    match schedule {
                        Err(lowering) => Err(DriverError::CodegenDiagnostics(lowering.diagnostics)),
                        Ok(mut schedule) => {
                            let mut emitter =
                                time_detail_stage(timings, mode.emitter_create_stage(), || {
                                    match mode {
                                        ObjectEmissionMode::NoLto => {
                                            nia_codegen_llvm::LlvmNativeObjectReadinessEmitter::new(
                                                schedule.module_store(),
                                                type_store,
                                                schedule.owner_directory(),
                                                options,
                                                object_cache,
                                                &session,
                                            )
                                            .map(ObjectReadinessEmitter::NoLto)
                                        }
                                        ObjectEmissionMode::Lto {
                                            lto_mode,
                                            freestanding,
                                            ..
                                        } => nia_codegen_llvm::LlvmLtoReadinessEmitter::new(
                                            schedule.module_store(),
                                            type_store,
                                            schedule.owner_directory(),
                                            options,
                                            nia_codegen_llvm::LtoPreLinkConfig {
                                                mode: lto_mode,
                                                freestanding,
                                            },
                                            lto_module_cache,
                                            &session,
                                        )
                                        .map(ObjectReadinessEmitter::Lto),
                                    }
                                })
                                .map_err(|error| {
                                    DriverError::InternalDiagnostic(Diagnostic::from(error))
                                })?;
                            loop {
                                let ready =
                                    time_detail_stage(timings, mode.backend_wait_stage(), || {
                                        schedule.wait_next()
                                    })
                                    .map_err(|error| {
                                        DriverError::InternalDiagnostic(query_error_diagnostic(
                                            error,
                                        ))
                                    })?;
                                let Some(ready) = ready else {
                                    break;
                                };
                                time_detail_stage(timings, mode.publish_ready_stage(), || {
                                    emitter.publish(ready)
                                })
                                .map_err(|error| {
                                    DriverError::InternalDiagnostic(Diagnostic::from(error))
                                })?;
                            }
                            let lowering =
                                time_detail_stage(timings, mode.backend_finish_stage(), || {
                                    schedule.finish()
                                })
                                .map_err(|error| {
                                    DriverError::InternalDiagnostic(query_error_diagnostic(error))
                                })?;
                            if !lowering.diagnostics.is_empty() {
                                return Err(DriverError::CodegenDiagnostics(lowering.diagnostics));
                            }
                            let backend_function_stats = lowering.program.function_stats();
                            let backend_module_count = lowering.program.modules.len();
                            Ok((
                                time_detail_stage(timings, mode.llvm_finish_stage(), || {
                                    emitter.finish(mode, options, lto_parallelism)
                                })
                                .map_err(|error| {
                                    DriverError::InternalDiagnostic(Diagnostic::from(error))
                                })?,
                                backend_function_stats,
                                backend_module_count,
                                lowering.optimization_report,
                            ))
                        }
                    }
                })())
            })
            .map_err(|error| DriverError::InternalDiagnostic(query_error_diagnostic(error)))??;
        if !output.diagnostics.is_empty() {
            return Err(DriverError::CodegenDiagnostics(output.diagnostics));
        }
        let link_input_count = output.link_inputs.len();
        Ok(NativeDatabaseEmission {
            artifact: ObjectArtifact {
                link_inputs: output.link_inputs,
                optimization,
                optimization_report,
                diagnostics,
            },
            checked_body_count,
            reachable_body_count: backend_function_stats.definitions(),
            backend_function_stats,
            link_input_count,
            checked_module_count,
            monomorphized_instance_count,
            backend_module_count,
        })
    }

    /// Emits native objects from an existing codegen product.
    pub fn emit_native_objects_from_codegen(
        &self,
        program: &CodegenProgram,
    ) -> DriverOutput<ObjectArtifact> {
        self.emit_native_objects_from_codegen_with_timings(program, TimingMode::Off)
    }

    /// Emits native objects while overriding timing mode.
    pub fn emit_native_objects_from_codegen_with_timings(
        &self,
        program: &CodegenProgram,
        timings: TimingMode,
    ) -> DriverOutput<ObjectArtifact> {
        {
            let session = match self.codegen_query_session() {
                Ok(session) => session,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            let cache = self.object_cache.as_ref().map(|cache| {
                cache.clone() as std::sync::Arc<dyn nia_codegen_llvm::ObjectWorkProductCache>
            });
            let output = nia_codegen_llvm::emit_native_objects(
                std::sync::Arc::clone(&program.backend_lowering),
                std::sync::Arc::clone(&program.type_store),
                &session,
                codegen_options(
                    program.optimization,
                    timings,
                    self.config.toolchain.identity().fingerprint(),
                ),
                cache,
            );
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            DriverOutput::success(ObjectArtifact {
                link_inputs: output.link_inputs,
                optimization: program.optimization,
                optimization_report: program.backend_lowering.optimization_report.clone(),
                diagnostics: program.diagnostics.clone(),
            })
        }
    }

    fn codegen_query_session(&self) -> nia_query::QueryResult<nia_query::QuerySession> {
        let compiler = self.compiler.lock().map_err(|_| {
            nia_query::QueryError::internal("driver compiler state lock is poisoned")
        })?;
        compiler
            .as_ref()
            .map(|compiler| compiler.database.query_session())
            .ok_or_else(|| {
                nia_query::QueryError::internal(
                    "LLVM emission requires the Driver that produced the codegen program",
                )
            })
    }

    /// Writes emitted native objects according to the request output policy.
    pub fn write_native_objects(
        &self,
        request: WriteObjectRequest,
    ) -> DriverOutput<WrittenObjectArtifact> {
        {
            let output = self.emit_native_objects(EmitObjectRequest {
                check: request.check,
            });
            let objects = match output.result {
                Ok(objects) => objects,
                Err(error) => return DriverOutput::from_error(error),
            };
            self.write_native_objects_from_artifact(&objects, request.output)
        }
    }

    /// Writes an existing object artifact to disk.
    pub fn write_native_objects_from_artifact(
        &self,
        objects: &ObjectArtifact,
        output: ObjectOutput,
    ) -> DriverOutput<WrittenObjectArtifact> {
        {
            let written = match output {
                ObjectOutput::Single(path) => {
                    if objects.link_inputs.len() != 1 {
                        return DriverOutput::from_error(DriverError::InvalidArtifactRequest(
                            "`-o` can only be used when the program has one codegen unit; use `--out-dir`"
                                .to_string(),
                        ));
                    }
                    let input = &objects.link_inputs.as_slice()[0];
                    if let Err(error) = write_output_file(&path, &input.object.bytes) {
                        return DriverOutput::from_error(DriverError::Io {
                            path,
                            operation: "write object file",
                            error,
                        });
                    }
                    vec![nia_codegen_llvm::IncrementalLinkInput {
                        key: input.key.clone(),
                        fingerprint: input.fingerprint,
                        object: path,
                    }]
                }
                ObjectOutput::Directory(dir) => {
                    if let Err(error) = fs::create_dir_all(&dir) {
                        return DriverOutput::from_error(DriverError::Io {
                            path: dir,
                            operation: "create object output directory",
                            error,
                        });
                    }
                    let mut paths = Vec::new();
                    for (index, input) in objects.link_inputs.as_slice().iter().enumerate() {
                        let path = dir.join(object_file_name(index, &input.object.name));
                        if let Err(error) = write_output_file(&path, &input.object.bytes) {
                            return DriverOutput::from_error(DriverError::Io {
                                path,
                                operation: "write object file",
                                error,
                            });
                        }
                        paths.push(nia_codegen_llvm::IncrementalLinkInput {
                            key: input.key.clone(),
                            fingerprint: input.fingerprint,
                            object: path,
                        });
                    }
                    paths
                }
            };
            let link_inputs = match nia_codegen_llvm::IncrementalLinkInputs::new(written) {
                Ok(inputs) => inputs,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        Diagnostic::from(error),
                    ));
                }
            };
            DriverOutput::success(WrittenObjectArtifact { link_inputs })
        }
    }

    fn loader_database(&self, request: &CheckRequest) -> nia_query::QueryResult<LoaderDatabase> {
        let key = LoaderKey {
            entry_path: request.entry_path.clone(),
            package_root: request.package_root.clone(),
            module_map: request.module_map.clone(),
            target: self.config.artifact_target.clone(),
            profile: request.profile,
            compilation_mode: request.compilation_mode,
            runtime: request.runtime.clone(),
        };
        let mut loader_guard = self
            .loader
            .lock()
            .map_err(|_| nia_query::QueryError::internal("driver loader state lock is poisoned"))?;
        let database = match &*loader_guard {
            Some(loader) if loader.key == key => loader.database.clone(),
            _ => {
                let mut load_request = LoadRequest::from_source_path(key.entry_path.clone())
                    .with_module_map(key.module_map.clone())
                    .with_sources(self.sources.clone())
                    .with_target(key.target.clone())
                    .with_profile(key.profile)
                    .with_compilation_mode(key.compilation_mode)
                    .with_runtime(key.runtime.clone())
                    .with_toolchain_layout(std::sync::Arc::clone(&self.config.toolchain))
                    .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
                    .with_frontend_cache_verification(self.config.verify_frontend_cache);
                if let Some(package_root) = &key.package_root {
                    load_request = load_request.with_package_root(package_root.clone());
                }
                let database = LoaderDatabase::new(load_request)?;
                *loader_guard = Some(SessionLoader {
                    key,
                    database: database.clone(),
                });
                database
            }
        };
        drop(loader_guard);
        Ok(database)
    }

    fn loader_query_trace(&self) -> nia_query::QueryResult<nia_query::QueryTrace> {
        let loader = self
            .loader
            .lock()
            .map_err(|_| nia_query::QueryError::internal("driver loader state lock is poisoned"))?;
        loader.as_ref().map_or_else(
            || Ok(nia_query::QueryTrace::default()),
            |loader| loader.database.query_trace(),
        )
    }
}

/// LLVM IR artifact and its semantic diagnostics/report.
#[derive(Debug, Clone, PartialEq)]
pub struct LlvmIrArtifact {
    /// Lowered LLVM modules.
    pub modules: Vec<nia_codegen_llvm::LlvmModuleOutput>,
    /// Effective optimization policy.
    pub optimization: OptimizationPolicy,
    /// Backend optimization changes.
    pub optimization_report: crate::BackendOptimizationReport,
    /// Warnings and non-fatal diagnostics.
    pub diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
}

/// Native object artifact and incremental link inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectArtifact {
    /// In-memory object inputs keyed for incremental linking.
    pub link_inputs: nia_codegen_llvm::IncrementalLinkInputs<nia_codegen_llvm::NativeObject>,
    /// Effective optimization policy.
    pub optimization: OptimizationPolicy,
    /// Backend optimization changes.
    pub optimization_report: crate::BackendOptimizationReport,
    /// Warnings and non-fatal diagnostics.
    pub diagnostics: Vec<ProgramDiagnostic>,
}

struct NativeDatabaseEmission {
    artifact: ObjectArtifact,
    checked_body_count: usize,
    reachable_body_count: usize,
    backend_function_stats: BackendFunctionStats,
    link_input_count: usize,
    checked_module_count: usize,
    monomorphized_instance_count: usize,
    backend_module_count: usize,
}

#[derive(Clone, Copy)]
enum ObjectEmissionMode<'a> {
    NoLto,
    Lto {
        lto_mode: nia_codegen_llvm::LtoMode,
        linkage_source_identity: &'a SourceIdentity,
        preserved_symbols: &'a [&'a str],
        freestanding: bool,
        backend_cache_directory: Option<&'a Path>,
    },
}

impl ObjectEmissionMode<'_> {
    fn select(
        self,
        no_lto: &'static str,
        thin_lto: &'static str,
        full_lto: &'static str,
    ) -> &'static str {
        match self {
            Self::NoLto => no_lto,
            Self::Lto {
                lto_mode: nia_codegen_llvm::LtoMode::Thin,
                ..
            } => thin_lto,
            Self::Lto {
                lto_mode: nia_codegen_llvm::LtoMode::Full,
                ..
            } => full_lto,
        }
    }

    fn prepare_database_stage(self) -> &'static str {
        self.select(
            "native_prepare_compilation_database",
            "thin_lto_prepare_compilation_database",
            "full_lto_prepare_compilation_database",
        )
    }

    fn emit_objects_stage(self) -> &'static str {
        self.select(
            "native_emit_objects",
            "thin_lto_emit_objects",
            "full_lto_emit_objects",
        )
    }

    fn loader_trace_stage(self) -> &'static str {
        self.select(
            "native_loader_query_trace",
            "thin_lto_loader_query_trace",
            "full_lto_loader_query_trace",
        )
    }

    fn emit_counters_stage(self) -> &'static str {
        self.select(
            "native_emit_counters",
            "thin_lto_emit_counters",
            "full_lto_emit_counters",
        )
    }

    fn source_manifest_stage(self) -> &'static str {
        self.select(
            "native_source_manifest",
            "thin_lto_source_manifest",
            "full_lto_source_manifest",
        )
    }

    fn codegen_preparation_stage(self) -> &'static str {
        self.select(
            "native_codegen_preparation",
            "thin_lto_codegen_preparation",
            "full_lto_codegen_preparation",
        )
    }

    fn emitter_create_stage(self) -> &'static str {
        self.select(
            "native_llvm_emitter_create",
            "thin_lto_llvm_emitter_create",
            "full_lto_llvm_emitter_create",
        )
    }

    fn backend_wait_stage(self) -> &'static str {
        self.select(
            "native_backend_wait_ready",
            "thin_lto_backend_wait_ready",
            "full_lto_backend_wait_ready",
        )
    }

    fn publish_ready_stage(self) -> &'static str {
        self.select(
            "native_llvm_publish_ready",
            "thin_lto_llvm_publish_ready",
            "full_lto_llvm_publish_ready",
        )
    }

    fn backend_finish_stage(self) -> &'static str {
        self.select(
            "native_backend_finish",
            "thin_lto_backend_finish",
            "full_lto_backend_finish",
        )
    }

    fn llvm_finish_stage(self) -> &'static str {
        self.select(
            "native_llvm_finish",
            "thin_lto_llvm_finish",
            "full_lto_llvm_finish",
        )
    }
}

enum ObjectReadinessEmitter<'session> {
    NoLto(nia_codegen_llvm::LlvmNativeObjectReadinessEmitter<'session>),
    Lto(nia_codegen_llvm::LlvmLtoReadinessEmitter<'session>),
}

impl ObjectReadinessEmitter<'_> {
    fn publish(&mut self, ready: nia_codegen_llvm::BackendModuleReady) -> nia_ice::IceResult<()> {
        match self {
            Self::NoLto(emitter) => emitter.publish(ready),
            Self::Lto(emitter) => emitter.publish(ready),
        }
    }

    fn finish(
        self,
        mode: ObjectEmissionMode<'_>,
        options: nia_codegen_llvm::LlvmCodegenOptions,
        parallelism: usize,
    ) -> nia_ice::IceResult<nia_codegen_llvm::LlvmObjectOutput> {
        match (self, mode) {
            (Self::NoLto(emitter), ObjectEmissionMode::NoLto) => emitter.finish(),
            (
                Self::Lto(emitter),
                ObjectEmissionMode::Lto {
                    lto_mode,
                    linkage_source_identity,
                    preserved_symbols,
                    backend_cache_directory,
                    ..
                },
            ) => {
                let modules = emitter.finish()?;
                Ok(match lto_mode {
                    nia_codegen_llvm::LtoMode::Thin => nia_codegen_llvm::emit_thin_lto_objects(
                        modules,
                        options,
                        nia_codegen_llvm::ThinLtoCodegenConfig {
                            parallelism,
                            preserved_symbols,
                            backend_cache_directory,
                        },
                    ),
                    nia_codegen_llvm::LtoMode::Full => nia_codegen_llvm::emit_full_lto_objects(
                        modules,
                        options,
                        nia_codegen_llvm::FullLtoCodegenConfig {
                            linkage_source_identity,
                            parallelism,
                            preserved_symbols,
                        },
                    ),
                })
            }
            _ => Err(nia_ice::Ice::new(
                "object readiness emitter does not match its emission mode",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ObjectArtifactWithSourceManifest {
    artifact: ObjectArtifact,
    source_manifest: SourceInputManifest,
}

/// Native object artifact after writing inputs to filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenObjectArtifact {
    /// Path-based incremental link inputs.
    pub link_inputs: nia_codegen_llvm::IncrementalLinkInputs<PathBuf>,
}

/// Linked executable artifact and its cache identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutableArtifact {
    /// Output executable path.
    pub path: PathBuf,
    /// Effective optimization policy.
    pub optimization: OptimizationPolicy,
    /// Backend optimization changes.
    pub optimization_report: crate::BackendOptimizationReport,
    /// Warnings and non-fatal diagnostics.
    pub diagnostics: Vec<ProgramDiagnostic>,
    /// Cache identity installed with this artifact, when enabled.
    pub cache_reference: Option<ExecutableCacheReference>,
}

/// Static archive artifact and its cache identity.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticArchiveArtifact {
    /// Output archive path.
    pub path: PathBuf,
    /// Effective optimization policy.
    pub optimization: OptimizationPolicy,
    /// Backend optimization changes.
    pub optimization_report: crate::BackendOptimizationReport,
    /// Warnings and non-fatal diagnostics.
    pub diagnostics: Vec<ProgramDiagnostic>,
    /// Cache identity installed with this artifact, when enabled.
    pub cache_reference: Option<StaticArchiveCacheReference>,
}

/// Driver operation result, including structured failure categories.
#[derive(Debug)]
pub struct DriverOutput<T> {
    /// Successful artifact/program or structured driver failure.
    pub result: Result<T, DriverError>,
}

impl<T> DriverOutput<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> DriverOutput<U> {
        DriverOutput {
            result: self.result.map(map),
        }
    }

    fn success(value: T) -> Self {
        Self { result: Ok(value) }
    }

    fn from_error(error: DriverError) -> Self {
        Self { result: Err(error) }
    }

    fn from_check_diagnostics(program: CheckedProgram) -> Self {
        Self::from_error(DriverError::CheckDiagnostics(program))
    }

    fn from_codegen_diagnostics(program: CodegenProgram) -> Self {
        Self::from_error(DriverError::CodegenProgramDiagnostics(Box::new(program)))
    }
}

/// Failures produced by orchestration, tools, diagnostics, or artifact I/O.
#[derive(Debug)]
pub enum DriverError {
    /// Archive tool exited unsuccessfully.
    ArchiveStatus {
        /// Tool program name.
        program: String,
        /// Process exit status.
        status: std::process::ExitStatus,
    },
    /// Archive tool could not be started or read.
    ArchiveIo {
        /// Tool program name.
        program: String,
        /// Underlying I/O error.
        error: io::Error,
    },
    /// Archive configuration was rejected before execution.
    ArchiveConfig(nia_linker::LinkerConfigError),
    /// Semantic checking produced user diagnostics.
    CheckDiagnostics(CheckedProgram),
    /// Codegen preparation retained a checked program with diagnostics.
    CodegenProgramDiagnostics(Box<CodegenProgram>),
    /// Codegen preparation diagnostics without a codegen product.
    CodegenPreparationDiagnostics(Vec<nia_compiler_query::ProgramDiagnostic>),
    /// Backend codegen diagnostics.
    CodegenDiagnostics(Vec<Diagnostic>),
    /// Internal compiler diagnostic or ICE conversion.
    InternalDiagnostic(Diagnostic),
    /// Request shape cannot produce the requested artifact.
    InvalidArtifactRequest(String),
    /// Runtime startup selection is unsupported for the artifact target.
    Runtime(nia_toolchain::RuntimeSpecError),
    /// Filesystem operation failed while publishing an artifact.
    Io {
        /// Destination path.
        path: PathBuf,
        /// Operation description.
        operation: &'static str,
        /// Underlying I/O error.
        error: io::Error,
    },
    /// Linker exited unsuccessfully.
    LinkerStatus {
        /// Tool program name.
        program: String,
        /// Process exit status.
        status: std::process::ExitStatus,
    },
    /// Linker could not be started or read.
    LinkerIo {
        /// Tool program name.
        program: String,
        /// Underlying I/O error.
        error: io::Error,
    },
    /// Linker configuration was rejected before execution.
    LinkerConfig(nia_linker::LinkerConfigError),
}

fn codegen_options(
    optimization: OptimizationPolicy,
    timings: TimingMode,
    toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
) -> nia_codegen_llvm::LlvmCodegenOptions {
    nia_codegen_llvm::LlvmCodegenOptions {
        optimization,
        timings,
        toolchain_identity,
    }
}
