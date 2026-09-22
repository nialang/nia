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
mod emission;
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
