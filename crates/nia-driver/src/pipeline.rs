// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    mem::size_of,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

use nia_compiler_query::{
    CompileRequest, CompilerDatabase, TimingMode, has_error_diagnostics, query_error_diagnostic,
};
use nia_diagnostic::Diagnostic;
use nia_imports::ModuleMap;
use nia_linker::{
    ArchiveCacheKey, ArchiveEnvironmentFingerprint, ArchiveFingerprint,
    ArchiveFingerprintComponents, ArchiveFingerprintSet, ArchiveOptions, LinkOptions,
    LinkResultCacheKey, LinkResultEnvironmentFingerprint, LinkResultFingerprint,
    LinkResultFingerprintComponents, LinkResultFingerprintSet, LinkTarget,
};
use nia_loader_query::{EntryRuntime, LoadRequest, LoaderDatabase, SourceInputManifest};
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_source::{SourceDatabase, SourcePath};
use nia_target_config::TargetConfig;
use nia_toolchain::ToolchainLayout;

use crate::{CheckedProgram, CodegenProgram, ProgramDiagnostic};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Runtime {
    #[default]
    Bare,
    Freestanding,
}

#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub entry_path: SourcePath,
    pub module_map: ModuleMap,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgramWithSourceManifest {
    pub program: CheckedProgram,
    pub source_manifest: SourceInputManifest,
}

const EXECUTABLE_CACHE_REFERENCE_LEN: usize = 12 * size_of::<u64>();
const EXECUTABLE_CACHE_ENVIRONMENT_LEN: usize = 6 * size_of::<u64>();
const STATIC_ARCHIVE_CACHE_REFERENCE_LEN: usize = 12 * size_of::<u64>();
const STATIC_ARCHIVE_CACHE_ENVIRONMENT_LEN: usize = size_of::<[u64; 8]>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableCacheEnvironment {
    fingerprint: LinkResultEnvironmentFingerprint,
}

impl ExecutableCacheEnvironment {
    pub const ENCODED_LEN: usize = EXECUTABLE_CACHE_ENVIRONMENT_LEN;

    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprint.target.parts(),
            self.fingerprint.linker.parts(),
            self.fingerprint.options.parts(),
        ] {
            for part in fingerprint {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableCacheReference {
    fingerprints: LinkResultFingerprintSet,
}

impl ExecutableCacheReference {
    pub const ENCODED_LEN: usize = EXECUTABLE_CACHE_REFERENCE_LEN;

    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprints.cache_key.parts(),
            self.fingerprints.components.inputs.parts(),
            self.fingerprints.components.toolchain.parts(),
            self.fingerprints.components.target.parts(),
            self.fingerprints.components.linker.parts(),
            self.fingerprints.components.options.parts(),
        ] {
            for part in fingerprint {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }

    pub fn decode(encoded: &[u8]) -> Option<Self> {
        (encoded.len() == Self::ENCODED_LEN).then_some(())?;
        let mut parts = encoded.chunks_exact(size_of::<u64>()).map(|bytes| {
            let bytes: [u8; size_of::<u64>()] = bytes.try_into().expect("exact u64 chunk");
            u64::from_le_bytes(bytes)
        });
        let mut fingerprint = || Some([parts.next()?, parts.next()?]);
        let cache_key = LinkResultCacheKey::from_parts(fingerprint()?);
        let components = LinkResultFingerprintComponents {
            inputs: LinkResultFingerprint::from_parts(fingerprint()?),
            toolchain: LinkResultFingerprint::from_parts(fingerprint()?),
            target: LinkResultFingerprint::from_parts(fingerprint()?),
            linker: LinkResultFingerprint::from_parts(fingerprint()?),
            options: LinkResultFingerprint::from_parts(fingerprint()?),
        };
        Some(Self {
            fingerprints: LinkResultFingerprintSet::new(cache_key, components),
        })
    }
}

impl From<LinkResultFingerprintSet> for ExecutableCacheReference {
    fn from(fingerprints: LinkResultFingerprintSet) -> Self {
        Self { fingerprints }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticArchiveCacheEnvironment {
    fingerprint: ArchiveEnvironmentFingerprint,
}

impl StaticArchiveCacheEnvironment {
    pub const ENCODED_LEN: usize = STATIC_ARCHIVE_CACHE_ENVIRONMENT_LEN;

    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprint.toolchain,
            self.fingerprint.target,
            self.fingerprint.tool,
            self.fingerprint.options,
        ] {
            for part in fingerprint.parts() {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticArchiveCacheReference {
    fingerprints: ArchiveFingerprintSet,
}

impl StaticArchiveCacheReference {
    pub const ENCODED_LEN: usize = STATIC_ARCHIVE_CACHE_REFERENCE_LEN;

    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            ArchiveFingerprint::from_parts(self.fingerprints.cache_key.parts()),
            self.fingerprints.components.inputs,
            self.fingerprints.components.toolchain,
            self.fingerprints.components.target,
            self.fingerprints.components.tool,
            self.fingerprints.components.options,
        ] {
            for part in fingerprint.parts() {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }

    pub fn decode(encoded: &[u8]) -> Option<Self> {
        (encoded.len() == Self::ENCODED_LEN).then_some(())?;
        let mut parts = encoded.chunks_exact(size_of::<u64>()).map(|bytes| {
            let bytes: [u8; size_of::<u64>()] = bytes.try_into().expect("exact u64 chunk");
            u64::from_le_bytes(bytes)
        });
        let mut fingerprint = || Some([parts.next()?, parts.next()?]);
        let cache_key = ArchiveCacheKey::from_parts(fingerprint()?);
        let components = ArchiveFingerprintComponents {
            inputs: ArchiveFingerprint::from_parts(fingerprint()?),
            toolchain: ArchiveFingerprint::from_parts(fingerprint()?),
            target: ArchiveFingerprint::from_parts(fingerprint()?),
            tool: ArchiveFingerprint::from_parts(fingerprint()?),
            options: ArchiveFingerprint::from_parts(fingerprint()?),
        };
        Some(Self {
            fingerprints: ArchiveFingerprintSet::new(cache_key, components),
        })
    }
}

impl From<ArchiveFingerprintSet> for StaticArchiveCacheReference {
    fn from(fingerprints: ArchiveFingerprintSet) -> Self {
        Self { fingerprints }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticArchiveCacheRestore {
    Hit,
    NotFound,
    Invalidated,
    Corrupt,
    ReadError,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableCacheRestore {
    Hit,
    NotFound,
    Invalidated,
    Corrupt,
    ReadError,
    Disabled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkedExecutableWithSourceManifest {
    pub artifact: ExecutableArtifact,
    pub source_manifest: SourceInputManifest,
}

#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub toolchain: std::sync::Arc<ToolchainLayout>,
    pub artifact_target: TargetConfig,
    pub artifact_cache_dir: Option<PathBuf>,
    pub verify_frontend_cache: bool,
}

impl DriverConfig {
    pub fn new(toolchain: std::sync::Arc<ToolchainLayout>) -> Self {
        let artifact_target = toolchain.artifact_target().clone();
        Self {
            toolchain,
            artifact_target,
            artifact_cache_dir: None,
            verify_frontend_cache: false,
        }
    }

    pub fn with_artifact_target(mut self, artifact_target: TargetConfig) -> Self {
        self.artifact_target = artifact_target;
        self
    }
}

#[derive(Debug)]
pub struct Driver {
    config: DriverConfig,
    sources: SourceDatabase,
    loader: std::sync::Arc<Mutex<Option<SessionLoader>>>,
    compiler: std::sync::Arc<Mutex<Option<SessionCompiler>>>,
    object_cache: Option<std::sync::Arc<crate::object_cache::PersistentObjectWorkProductCache>>,
    link_cache: Option<std::sync::Arc<crate::executable_cache::PersistentLinkResultCache>>,
    archive_cache: Option<std::sync::Arc<crate::archive_cache::PersistentArchiveCache>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkResultReuse {
    Hit,
    Miss(LinkResultReuseMiss),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkResultReuseMiss {
    Disabled,
    Uncacheable,
    NotFound,
    Invalidated(nia_linker::LinkResultInvalidation),
    Corrupt,
    ReadError,
}

fn emit_link_result_reuse(timings: TimingMode, reuse: LinkResultReuse) {
    if !timings.enabled() {
        return;
    }
    let hit = u64::from(reuse == LinkResultReuse::Hit);
    nia_timing::emit_counter("link.result_reuse_hits", hit);
    nia_timing::emit_counter("link.result_reuse_misses", 1 - hit);
    for (name, reason) in [
        (
            "link.result_reuse_miss_disabled",
            LinkResultReuseMiss::Disabled,
        ),
        (
            "link.result_reuse_miss_uncacheable",
            LinkResultReuseMiss::Uncacheable,
        ),
        (
            "link.result_reuse_miss_not_found",
            LinkResultReuseMiss::NotFound,
        ),
        (
            "link.result_reuse_miss_corrupt",
            LinkResultReuseMiss::Corrupt,
        ),
        (
            "link.result_reuse_miss_read_error",
            LinkResultReuseMiss::ReadError,
        ),
    ] {
        nia_timing::emit_counter(name, u64::from(reuse == LinkResultReuse::Miss(reason)));
    }
    let invalidation = match reuse {
        LinkResultReuse::Miss(LinkResultReuseMiss::Invalidated(reasons)) => Some(reasons),
        _ => None,
    };
    nia_timing::emit_counter(
        "link.result_reuse_miss_invalidated",
        u64::from(invalidation.is_some()),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_inputs",
        u64::from(invalidation.is_some_and(|reasons| reasons.inputs)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_toolchain",
        u64::from(invalidation.is_some_and(|reasons| reasons.toolchain)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_target",
        u64::from(invalidation.is_some_and(|reasons| reasons.target)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_linker",
        u64::from(invalidation.is_some_and(|reasons| reasons.linker)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_options",
        u64::from(invalidation.is_some_and(|reasons| reasons.options)),
    );
}

impl Driver {
    pub fn new(toolchain: std::sync::Arc<ToolchainLayout>) -> Self {
        Self::with_config(DriverConfig::new(toolchain))
    }

    pub fn with_config(config: DriverConfig) -> Self {
        let object_cache = config.artifact_cache_dir.as_ref().map(|path| {
            std::sync::Arc::new(crate::object_cache::PersistentObjectWorkProductCache::new(
                path.clone(),
            ))
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
            link_cache,
            archive_cache,
        }
    }

    pub fn config(&self) -> &DriverConfig {
        &self.config
    }

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

    pub fn set_source(&self, path: impl Into<String>, text: impl Into<std::sync::Arc<str>>) {
        let path = path.into();
        let loader = self.loader.lock().expect("driver loader lock poisoned");
        if let Some(loader) = &*loader {
            loader.database.set_source(path, text);
        } else {
            drop(loader);
            self.sources.set_source(SourcePath::new(path), text);
        }
    }

    pub fn check_all_modules(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        DriverOutput::catch_ice(|| {
            let program = match self.check_all_modules_inner(request) {
                Ok(program) => program,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if has_error_diagnostics(&program.diagnostics) {
                return DriverOutput::from_check_diagnostics(program);
            }
            DriverOutput::success(program)
        })
    }

    pub fn source_input_manifest(
        &self,
        request: &CheckRequest,
    ) -> DriverOutput<SourceInputManifest> {
        DriverOutput::catch_ice(|| {
            let loader = self.loader_database(request);
            if let Err(error) = loader.load_program() {
                return DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                ));
            }
            match loader.source_input_manifest() {
                Ok(manifest) => DriverOutput::success(manifest),
                Err(error) => DriverOutput::from_error(DriverError::InternalDiagnostic(
                    query_error_diagnostic(error),
                )),
            }
        })
    }

    fn check_all_modules_inner(
        &self,
        request: CheckRequest,
    ) -> nia_query::QueryResult<CheckedProgram> {
        self.compile_with(request, CompilerDatabase::check_program)
    }

    #[cfg(test)]
    pub(crate) fn analyze_all_modules(
        &self,
        request: CheckRequest,
    ) -> nia_compiler_query::CheckedProgramAnalysis {
        self.compile_with(request, CompilerDatabase::analyze_program)
            .expect("test compiler analysis")
    }

    pub fn check_entry(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        DriverOutput::catch_ice(|| {
            let program = match self.check_entry_inner(request) {
                Ok(program) => program,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if has_error_diagnostics(&program.diagnostics) {
                return DriverOutput::from_check_diagnostics(program);
            }
            DriverOutput::success(program)
        })
    }

    pub fn check_entry_with_source_manifest(
        &self,
        request: CheckRequest,
    ) -> DriverOutput<CheckedProgramWithSourceManifest> {
        DriverOutput::catch_ice(|| {
            let checked = match self.check_entry_with_source_manifest_inner(request) {
                Ok(checked) => checked,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if has_error_diagnostics(&checked.program.diagnostics) {
                return DriverOutput::from_check_diagnostics(checked.program);
            }
            DriverOutput::success(checked)
        })
    }

    fn check_entry_inner(&self, request: CheckRequest) -> nia_query::QueryResult<CheckedProgram> {
        self.compile_with(request, CompilerDatabase::entry_check_program)
    }

    fn check_entry_with_source_manifest_inner(
        &self,
        request: CheckRequest,
    ) -> nia_query::QueryResult<CheckedProgramWithSourceManifest> {
        let (program, source_manifest) =
            self.compile_with_source_manifest(request, CompilerDatabase::entry_check_program)?;
        Ok(CheckedProgramWithSourceManifest {
            program,
            source_manifest,
        })
    }

    #[cfg(test)]
    pub(crate) fn analyze_entry_program(
        &self,
        request: CheckRequest,
    ) -> nia_compiler_query::CheckedProgramAnalysis {
        self.compile_with(request, CompilerDatabase::analyze_entry_program)
            .expect("test entry compiler analysis")
    }

    pub fn codegen(&self, request: CheckRequest) -> DriverOutput<CodegenProgram> {
        DriverOutput::catch_ice(|| {
            let program = match self.codegen_inner(request) {
                Ok(program) => program,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            if has_error_diagnostics(&program.diagnostics) {
                return DriverOutput::from_codegen_diagnostics(program);
            }
            DriverOutput::success(program)
        })
    }

    fn codegen_inner(&self, request: CheckRequest) -> nia_query::QueryResult<CodegenProgram> {
        self.compile_with(request, CompilerDatabase::codegen_program)
    }

    fn compile_with<T>(
        &self,
        request: CheckRequest,
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<T>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let database = self.compiler_database(&request)?;
        let output = compile(&database)?;
        let loader_trace = self.loader_query_trace();
        emit_compilation_counters(
            timings,
            &database,
            &loader_trace,
            &output,
            database.provider_demand_rounds(),
        );
        Ok(output)
    }

    fn compile_with_source_manifest<T>(
        &self,
        request: CheckRequest,
        compile: impl Fn(&CompilerDatabase) -> nia_query::QueryResult<T>,
    ) -> nia_query::QueryResult<(T, SourceInputManifest)>
    where
        T: ProviderDemandOutput,
    {
        let timings = request.timings;
        let (database, loader) = self.compilation_databases(&request)?;
        let output = compile(&database)?;
        let source_manifest = loader.source_input_manifest()?;
        let loader_trace = self.loader_query_trace();
        emit_compilation_counters(
            timings,
            &database,
            &loader_trace,
            &output,
            database.provider_demand_rounds(),
        );
        Ok((output, source_manifest))
    }

    fn compiler_database(
        &self,
        request: &CheckRequest,
    ) -> nia_query::QueryResult<CompilerDatabase> {
        self.compilation_databases(request)
            .map(|(compiler, _)| compiler)
    }

    fn compilation_databases(
        &self,
        request: &CheckRequest,
    ) -> nia_query::QueryResult<(CompilerDatabase, LoaderDatabase)> {
        let loader = self.loader_database(request);
        loader.load_program()?;
        let query_session = loader.query_session();
        let mut compiler_guard = self.compiler.lock().expect("driver compiler lock poisoned");
        let database = if let Some(compiler) = &*compiler_guard
            && compiler.database.query_session().ptr_eq(&query_session)
        {
            compiler.database.update(
                CompileRequest::new(loader.clone())
                    .with_optimization(request.optimization)
                    .with_timings(request.timings)
                    .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
                    .with_frontend_cache_verification(self.config.verify_frontend_cache),
            )?;
            compiler.database.clone()
        } else {
            let database = CompilerDatabase::new(
                CompileRequest::new(loader.clone())
                    .with_optimization(request.optimization)
                    .with_timings(request.timings)
                    .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
                    .with_frontend_cache_verification(self.config.verify_frontend_cache),
            );
            *compiler_guard = Some(SessionCompiler {
                database: database.clone(),
            });
            database
        };
        drop(compiler_guard);
        Ok((database, loader))
    }

    pub fn emit_llvm_ir(&self, request: EmitLlvmRequest) -> DriverOutput<LlvmIrArtifact> {
        DriverOutput::catch_ice(|| {
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
            let options = codegen_options(
                preparation.optimization,
                timings,
                self.config.toolchain.identity().fingerprint(),
            );
            let optimization = preparation.optimization;
            let diagnostics = preparation.diagnostics;
            let type_store = std::sync::Arc::clone(&preparation.type_store);
            let session = database.query_session();
            let result = database.with_backend_finalization_schedule(|schedule| match schedule {
                Err(lowering) => Err(DriverError::CodegenDiagnostics(lowering.diagnostics)),
                Ok(mut schedule) => {
                    let mut emitter = nia_codegen_llvm::LlvmIrReadinessEmitter::new(
                        schedule.module_store(),
                        type_store,
                        schedule.owner_directory(),
                        options,
                        &session,
                    );
                    while let Some(ready) = schedule.wait_next().map_err(|error| {
                        DriverError::InternalDiagnostic(query_error_diagnostic(error))
                    })? {
                        emitter.publish(ready);
                    }
                    let lowering = schedule.finish().map_err(|error| {
                        DriverError::InternalDiagnostic(query_error_diagnostic(error))
                    })?;
                    if !lowering.diagnostics.is_empty() {
                        return Err(DriverError::CodegenDiagnostics(lowering.diagnostics));
                    }
                    let reachable_body_count = lowering
                        .program
                        .modules
                        .iter()
                        .map(|module| module.functions.len() + module.function_instances.len())
                        .sum();
                    Ok((
                        emitter.finish(),
                        reachable_body_count,
                        lowering.optimization_report,
                    ))
                }
            });
            let (output, reachable_body_count, optimization_report) = match result {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => return DriverOutput::from_error(error),
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            let loader_trace = self.loader_query_trace();
            emit_compilation_counters(
                timings,
                &database,
                &loader_trace,
                &LiveCodegenCounters {
                    checked_body_count,
                    reachable_body_count,
                },
                database.provider_demand_rounds(),
            );
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
        })
    }

    pub fn emit_llvm_ir_from_codegen(
        &self,
        program: &CodegenProgram,
    ) -> DriverOutput<LlvmIrArtifact> {
        self.emit_llvm_ir_from_codegen_with_timings(program, TimingMode::Off)
    }

    pub fn emit_llvm_ir_from_codegen_with_timings(
        &self,
        program: &CodegenProgram,
        timings: TimingMode,
    ) -> DriverOutput<LlvmIrArtifact> {
        DriverOutput::catch_ice(|| {
            let session = self.codegen_query_session();
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
        })
    }

    pub fn emit_native_objects(&self, request: EmitObjectRequest) -> DriverOutput<ObjectArtifact> {
        self.emit_native_objects_with_source_manifest(request)
            .map(|output| output.artifact)
    }

    fn emit_native_objects_with_source_manifest(
        &self,
        request: EmitObjectRequest,
    ) -> DriverOutput<ObjectArtifactWithSourceManifest> {
        DriverOutput::catch_ice(|| {
            let timings = request.check.timings;
            let (database, loader) = match self.compilation_databases(&request.check) {
                Ok(databases) => databases,
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
            let optimization = preparation.optimization;
            let diagnostics = preparation.diagnostics;
            let type_store = std::sync::Arc::clone(&preparation.type_store);
            let options = codegen_options(
                optimization,
                timings,
                self.config.toolchain.identity().fingerprint(),
            );
            let session = database.query_session();
            let cache = self.object_cache.as_ref().map(|cache| {
                cache.clone() as std::sync::Arc<dyn nia_codegen_llvm::ObjectWorkProductCache>
            });
            let result = database.with_backend_finalization_schedule(|schedule| match schedule {
                Err(lowering) => Err(DriverError::CodegenDiagnostics(lowering.diagnostics)),
                Ok(mut schedule) => {
                    let mut emitter = nia_codegen_llvm::LlvmNativeObjectReadinessEmitter::new(
                        schedule.module_store(),
                        type_store,
                        schedule.owner_directory(),
                        options,
                        cache,
                        &session,
                    );
                    while let Some(ready) = schedule.wait_next().map_err(|error| {
                        DriverError::InternalDiagnostic(query_error_diagnostic(error))
                    })? {
                        emitter.publish(ready);
                    }
                    let lowering = schedule.finish().map_err(|error| {
                        DriverError::InternalDiagnostic(query_error_diagnostic(error))
                    })?;
                    if !lowering.diagnostics.is_empty() {
                        return Err(DriverError::CodegenDiagnostics(lowering.diagnostics));
                    }
                    let reachable_body_count = lowering
                        .program
                        .modules
                        .iter()
                        .map(|module| module.functions.len() + module.function_instances.len())
                        .sum();
                    Ok((
                        emitter.finish(),
                        reachable_body_count,
                        lowering.optimization_report,
                    ))
                }
            });
            let (output, reachable_body_count, optimization_report) = match result {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => return DriverOutput::from_error(error),
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            let loader_trace = self.loader_query_trace();
            emit_compilation_counters(
                timings,
                &database,
                &loader_trace,
                &LiveCodegenCounters {
                    checked_body_count,
                    reachable_body_count,
                },
                database.provider_demand_rounds(),
            );
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            let source_manifest = match loader.source_input_manifest() {
                Ok(manifest) => manifest,
                Err(error) => {
                    return DriverOutput::from_error(DriverError::InternalDiagnostic(
                        query_error_diagnostic(error),
                    ));
                }
            };
            DriverOutput::success(ObjectArtifactWithSourceManifest {
                artifact: ObjectArtifact {
                    link_inputs: output.link_inputs,
                    optimization,
                    optimization_report,
                    diagnostics,
                },
                source_manifest,
            })
        })
    }

    pub fn emit_native_objects_from_codegen(
        &self,
        program: &CodegenProgram,
    ) -> DriverOutput<ObjectArtifact> {
        self.emit_native_objects_from_codegen_with_timings(program, TimingMode::Off)
    }

    pub fn emit_native_objects_from_codegen_with_timings(
        &self,
        program: &CodegenProgram,
        timings: TimingMode,
    ) -> DriverOutput<ObjectArtifact> {
        DriverOutput::catch_ice(|| {
            let session = self.codegen_query_session();
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
        })
    }

    fn codegen_query_session(&self) -> nia_query::QuerySession {
        self.compiler
            .lock()
            .expect("driver compiler lock poisoned")
            .as_ref()
            .expect("Nia ICE: LLVM emission requires the Driver that produced the codegen program")
            .database
            .query_session()
    }

    pub fn write_native_objects(
        &self,
        request: WriteObjectRequest,
    ) -> DriverOutput<WrittenObjectArtifact> {
        DriverOutput::catch_ice(|| {
            let output = self.emit_native_objects(EmitObjectRequest {
                check: request.check,
            });
            let objects = match output.result {
                Ok(objects) => objects,
                Err(error) => return DriverOutput::from_error(error),
            };
            self.write_native_objects_from_artifact(&objects, request.output)
        })
    }

    pub fn write_native_objects_from_artifact(
        &self,
        objects: &ObjectArtifact,
        output: ObjectOutput,
    ) -> DriverOutput<WrittenObjectArtifact> {
        DriverOutput::catch_ice(|| {
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
            DriverOutput::success(WrittenObjectArtifact {
                link_inputs: nia_codegen_llvm::IncrementalLinkInputs::new(written),
            })
        })
    }

    pub fn link_executable(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<ExecutableArtifact> {
        self.link_executable_with_source_manifest(request)
            .map(|output| output.artifact)
    }

    pub fn link_executable_with_source_manifest(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<LinkedExecutableWithSourceManifest> {
        DriverOutput::catch_ice(|| {
            let mut request = request;
            request.link_options.target =
                LinkTarget::from_target_config(&self.config.artifact_target);
            let timings = request.check.timings;
            let output = nia_timing::time_stage(
                timings,
                nia_timing::TimingLevel::Summary,
                "emit_native_objects",
                || {
                    self.emit_native_objects_with_source_manifest(EmitObjectRequest {
                        check: request.check.with_runtime(Runtime::Freestanding),
                    })
                },
            );
            let objects = match output.result {
                Ok(objects) => objects,
                Err(error) => return DriverOutput::from_error(error),
            };
            let linked = self.link_executable_from_objects(
                &objects.artifact,
                request.output,
                request.link_options,
                timings,
            );
            match linked.result {
                Ok(artifact) => DriverOutput::success(LinkedExecutableWithSourceManifest {
                    artifact,
                    source_manifest: objects.source_manifest,
                }),
                Err(error) => DriverOutput::from_error(error),
            }
        })
    }

    pub fn link_executable_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        mut link_options: LinkOptions,
        timings: TimingMode,
    ) -> DriverOutput<ExecutableArtifact> {
        DriverOutput::catch_ice(|| {
            link_options.target = LinkTarget::from_target_config(&self.config.artifact_target);
            let link_fingerprint = match link_options.result_fingerprint(
                &objects.link_inputs,
                self.config.toolchain.identity().fingerprint(),
            ) {
                Ok(fingerprint) => fingerprint,
                Err(error) => return DriverOutput::from_error(DriverError::LinkerConfig(error)),
            };
            let reuse = match (&self.link_cache, link_fingerprint) {
                (None, _) => LinkResultReuse::Miss(LinkResultReuseMiss::Disabled),
                (Some(_), None) => LinkResultReuse::Miss(LinkResultReuseMiss::Uncacheable),
                (Some(cache), Some(fingerprint)) => match cache.restore(fingerprint, &output) {
                    Ok(crate::executable_cache::LinkResultCacheLookup::Hit) => LinkResultReuse::Hit,
                    Ok(crate::executable_cache::LinkResultCacheLookup::NotFound) => {
                        LinkResultReuse::Miss(LinkResultReuseMiss::NotFound)
                    }
                    Ok(crate::executable_cache::LinkResultCacheLookup::Invalidated(reasons)) => {
                        LinkResultReuse::Miss(LinkResultReuseMiss::Invalidated(reasons))
                    }
                    Ok(crate::executable_cache::LinkResultCacheLookup::Corrupt) => {
                        LinkResultReuse::Miss(LinkResultReuseMiss::Corrupt)
                    }
                    Err(_) => LinkResultReuse::Miss(LinkResultReuseMiss::ReadError),
                },
            };
            emit_link_result_reuse(timings, reuse);
            if reuse == LinkResultReuse::Hit {
                return DriverOutput::success(ExecutableArtifact {
                    path: output,
                    optimization: objects.optimization,
                    optimization_report: objects.optimization_report.clone(),
                    diagnostics: objects.diagnostics.clone(),
                    cache_reference: link_fingerprint.map(ExecutableCacheReference::from),
                });
            }
            let temp = TempDir::new("nia_emit_exe");
            if let Err(error) = fs::create_dir_all(temp.path()) {
                return DriverOutput::from_error(DriverError::Io {
                    path: temp.path().to_path_buf(),
                    operation: "create temporary object directory",
                    error,
                });
            }
            let mut link_inputs = Vec::new();
            for (index, input) in objects.link_inputs.as_slice().iter().enumerate() {
                let object_path = temp
                    .path()
                    .join(object_file_name(index, &input.object.name));
                if let Err(error) = write_output_file(&object_path, &input.object.bytes) {
                    return DriverOutput::from_error(DriverError::Io {
                        path: object_path,
                        operation: "write temporary object file",
                        error,
                    });
                }
                link_inputs.push(nia_codegen_llvm::IncrementalLinkInput {
                    key: input.key.clone(),
                    fingerprint: input.fingerprint,
                    object: object_path,
                });
            }
            let link_inputs = nia_codegen_llvm::IncrementalLinkInputs::new(link_inputs);
            if let Some(parent) = output.parent()
                && !parent.as_os_str().is_empty()
                && let Err(error) = fs::create_dir_all(parent)
            {
                return DriverOutput::from_error(DriverError::Io {
                    path: parent.to_path_buf(),
                    operation: "create executable output directory",
                    error,
                });
            }

            let invocation = match link_options.invocation(&link_inputs, output.clone()) {
                Ok(invocation) => invocation,
                Err(error) => return DriverOutput::from_error(DriverError::LinkerConfig(error)),
            };
            match Command::new(&invocation.program)
                .args(&invocation.args)
                .status()
            {
                Ok(status) if status.success() => {
                    let cache_reference = if let (Some(cache), Some(fingerprint)) =
                        (&self.link_cache, link_fingerprint)
                    {
                        let publish_error = cache.publish(fingerprint, &output).is_err();
                        if timings.enabled() {
                            nia_timing::emit_counter(
                                "link.result_cache_publish_errors",
                                u64::from(publish_error),
                            );
                        }
                        (!publish_error).then(|| ExecutableCacheReference::from(fingerprint))
                    } else {
                        None
                    };
                    DriverOutput::success(ExecutableArtifact {
                        path: output,
                        optimization: objects.optimization,
                        optimization_report: objects.optimization_report.clone(),
                        diagnostics: objects.diagnostics.clone(),
                        cache_reference,
                    })
                }
                Ok(status) => DriverOutput::from_error(DriverError::LinkerStatus {
                    program: invocation.program,
                    status,
                }),
                Err(error) => DriverOutput::from_error(DriverError::LinkerIo {
                    program: invocation.program,
                    error,
                }),
            }
        })
    }

    pub fn archive_static_library_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        mut archive_options: ArchiveOptions,
    ) -> DriverOutput<StaticArchiveArtifact> {
        DriverOutput::catch_ice(|| {
            archive_options.target = LinkTarget::from_target_config(&self.config.artifact_target);
            let archive_fingerprint = match archive_options.result_fingerprint(
                &objects.link_inputs,
                self.config.toolchain.identity().fingerprint(),
            ) {
                Ok(fingerprint) => fingerprint,
                Err(error) => return DriverOutput::from_error(DriverError::ArchiveConfig(error)),
            };
            if let Some(cache) = &self.archive_cache
                && matches!(
                    cache.restore(archive_fingerprint, &output),
                    Ok(crate::archive_cache::ArchiveCacheLookup::Hit)
                )
            {
                return DriverOutput::success(StaticArchiveArtifact {
                    path: output,
                    optimization: objects.optimization,
                    optimization_report: objects.optimization_report.clone(),
                    diagnostics: objects.diagnostics.clone(),
                    cache_reference: Some(StaticArchiveCacheReference::from(archive_fingerprint)),
                });
            }
            let temp = TempDir::new("nia_archive");
            if let Err(error) = fs::create_dir_all(temp.path()) {
                return DriverOutput::from_error(DriverError::Io {
                    path: temp.path().to_path_buf(),
                    operation: "create temporary archive directory",
                    error,
                });
            }
            let mut inputs = Vec::with_capacity(objects.link_inputs.len());
            for (index, input) in objects.link_inputs.as_slice().iter().enumerate() {
                let object_path = temp
                    .path()
                    .join(archive_member_file_name(index, &input.key));
                if let Err(error) = write_output_file(&object_path, &input.object.bytes) {
                    return DriverOutput::from_error(DriverError::Io {
                        path: object_path,
                        operation: "write temporary archive member",
                        error,
                    });
                }
                inputs.push(object_path);
            }
            let temporary_archive = temp.path().join("output.a");
            let invocation = match archive_options.invocation(&inputs, temporary_archive.clone()) {
                Ok(invocation) => invocation,
                Err(error) => return DriverOutput::from_error(DriverError::ArchiveConfig(error)),
            };
            match Command::new(&invocation.program)
                .args(&invocation.args)
                .status()
            {
                Ok(status) if status.success() => {
                    let bytes = match fs::read(&temporary_archive) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            return DriverOutput::from_error(DriverError::Io {
                                path: temporary_archive,
                                operation: "read temporary static archive",
                                error,
                            });
                        }
                    };
                    if let Err(error) = write_output_file(&output, &bytes) {
                        return DriverOutput::from_error(DriverError::Io {
                            path: output,
                            operation: "write static archive",
                            error,
                        });
                    }
                    let cache_reference = self.archive_cache.as_ref().and_then(|cache| {
                        cache
                            .publish(archive_fingerprint, &output)
                            .ok()
                            .map(|()| StaticArchiveCacheReference::from(archive_fingerprint))
                    });
                    DriverOutput::success(StaticArchiveArtifact {
                        path: output,
                        optimization: objects.optimization,
                        optimization_report: objects.optimization_report.clone(),
                        diagnostics: objects.diagnostics.clone(),
                        cache_reference,
                    })
                }
                Ok(status) => DriverOutput::from_error(DriverError::ArchiveStatus {
                    program: invocation.program,
                    status,
                }),
                Err(error) => DriverOutput::from_error(DriverError::ArchiveIo {
                    program: invocation.program,
                    error,
                }),
            }
        })
    }

    pub fn restore_executable_cache(
        &self,
        reference: ExecutableCacheReference,
        output: &Path,
    ) -> ExecutableCacheRestore {
        let Some(cache) = &self.link_cache else {
            return ExecutableCacheRestore::Disabled;
        };
        let options = LinkOptions {
            target: LinkTarget::from_target_config(&self.config.artifact_target),
            ..LinkOptions::default()
        };
        if !matches!(
            options.matches_result_environment(
                reference.fingerprints.components,
                self.config.toolchain.identity().fingerprint(),
            ),
            Ok(true)
        ) {
            return ExecutableCacheRestore::Invalidated;
        }
        match cache.restore(reference.fingerprints, output) {
            Ok(lookup) => match lookup {
                crate::executable_cache::LinkResultCacheLookup::Hit => ExecutableCacheRestore::Hit,
                crate::executable_cache::LinkResultCacheLookup::NotFound => {
                    ExecutableCacheRestore::NotFound
                }
                crate::executable_cache::LinkResultCacheLookup::Invalidated(_) => {
                    ExecutableCacheRestore::Invalidated
                }
                crate::executable_cache::LinkResultCacheLookup::Corrupt => {
                    ExecutableCacheRestore::Corrupt
                }
            },
            Err(_) => ExecutableCacheRestore::ReadError,
        }
    }

    pub fn executable_cache_environment(&self) -> Option<ExecutableCacheEnvironment> {
        self.executable_cache_environment_for(&LinkOptions::default())
    }

    pub fn executable_cache_environment_for(
        &self,
        link_options: &LinkOptions,
    ) -> Option<ExecutableCacheEnvironment> {
        self.link_cache.as_ref()?;
        let options = LinkOptions {
            target: LinkTarget::from_target_config(&self.config.artifact_target),
            ..link_options.clone()
        };
        options
            .result_environment_fingerprint(self.config.toolchain.identity().fingerprint())
            .ok()?
            .map(|fingerprint| ExecutableCacheEnvironment { fingerprint })
    }

    pub fn restore_static_archive_cache(
        &self,
        reference: StaticArchiveCacheReference,
        output: &Path,
    ) -> StaticArchiveCacheRestore {
        let Some(cache) = &self.archive_cache else {
            return StaticArchiveCacheRestore::Disabled;
        };
        let options = ArchiveOptions {
            target: LinkTarget::from_target_config(&self.config.artifact_target),
            ..ArchiveOptions::default()
        };
        if !matches!(
            options.matches_result_environment(
                reference.fingerprints.components,
                self.config.toolchain.identity().fingerprint(),
            ),
            Ok(true)
        ) {
            return StaticArchiveCacheRestore::Invalidated;
        }
        match cache.restore(reference.fingerprints, output) {
            Ok(lookup) => match lookup {
                crate::archive_cache::ArchiveCacheLookup::Hit => StaticArchiveCacheRestore::Hit,
                crate::archive_cache::ArchiveCacheLookup::NotFound => {
                    StaticArchiveCacheRestore::NotFound
                }
                crate::archive_cache::ArchiveCacheLookup::Invalidated(_) => {
                    StaticArchiveCacheRestore::Invalidated
                }
                crate::archive_cache::ArchiveCacheLookup::Corrupt => {
                    StaticArchiveCacheRestore::Corrupt
                }
            },
            Err(_) => StaticArchiveCacheRestore::ReadError,
        }
    }

    pub fn static_archive_cache_environment(&self) -> Option<StaticArchiveCacheEnvironment> {
        self.archive_cache.as_ref()?;
        let options = ArchiveOptions {
            target: LinkTarget::from_target_config(&self.config.artifact_target),
            ..ArchiveOptions::default()
        };
        options
            .environment_fingerprint(self.config.toolchain.identity().fingerprint())
            .ok()
            .map(|fingerprint| StaticArchiveCacheEnvironment { fingerprint })
    }

    fn loader_database(&self, request: &CheckRequest) -> LoaderDatabase {
        let key = LoaderKey {
            entry_path: request.entry_path.clone(),
            module_map: request.module_map.clone(),
            target: self.config.artifact_target.clone(),
            entry_runtime: entry_runtime(request.runtime),
        };
        let mut loader_guard = self.loader.lock().expect("driver loader lock poisoned");
        let database = match &*loader_guard {
            Some(loader) if loader.key == key => loader.database.clone(),
            _ => {
                let database = LoaderDatabase::new(
                    LoadRequest::from_source_path(key.entry_path.clone())
                        .with_module_map(key.module_map.clone())
                        .with_sources(self.sources.clone())
                        .with_target(key.target.clone())
                        .with_entry_runtime(key.entry_runtime)
                        .with_toolchain_layout(std::sync::Arc::clone(&self.config.toolchain))
                        .with_frontend_cache_dir(self.config.artifact_cache_dir.clone())
                        .with_frontend_cache_verification(self.config.verify_frontend_cache),
                );
                *loader_guard = Some(SessionLoader {
                    key,
                    database: database.clone(),
                });
                database
            }
        };
        drop(loader_guard);
        database
    }

    fn loader_query_trace(&self) -> nia_query::QueryTrace {
        self.loader
            .lock()
            .expect("driver loader lock poisoned")
            .as_ref()
            .map(|loader| loader.database.query_trace())
            .unwrap_or_default()
    }
}

trait ProviderDemandOutput {
    fn checked_body_count(&self) -> usize;
    fn reachable_body_count(&self) -> usize;
}

struct LiveCodegenCounters {
    checked_body_count: usize,
    reachable_body_count: usize,
}

impl ProviderDemandOutput for LiveCodegenCounters {
    fn checked_body_count(&self) -> usize {
        self.checked_body_count
    }

    fn reachable_body_count(&self) -> usize {
        self.reachable_body_count
    }
}

impl ProviderDemandOutput for CheckedProgram {
    fn checked_body_count(&self) -> usize {
        self.checked_body_count()
    }

    fn reachable_body_count(&self) -> usize {
        self.reachable_body_count()
    }
}

#[cfg(test)]
impl ProviderDemandOutput for nia_compiler_query::CheckedProgramAnalysis {
    fn checked_body_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum()
    }

    fn reachable_body_count(&self) -> usize {
        self.checked_body_count()
    }
}

impl ProviderDemandOutput for CodegenProgram {
    fn checked_body_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum()
    }

    fn reachable_body_count(&self) -> usize {
        self.backend_lowering
            .program
            .modules
            .iter()
            .map(|module| module.functions.len() + module.function_instances.len())
            .sum()
    }
}

fn emit_compilation_counters(
    timings: TimingMode,
    database: &CompilerDatabase,
    loader_trace: &nia_query::QueryTrace,
    output: &impl ProviderDemandOutput,
    provider_demand_rounds: u64,
) {
    if !timings.enabled() {
        return;
    }
    let compiler_trace = database.query_trace();
    let traces = [loader_trace, &compiler_trace];
    nia_timing::emit_counter(
        "query.executions",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.executions as u64)
            .sum(),
    );
    nia_timing::emit_counter(
        "query.cache_hits",
        traces
            .iter()
            .flat_map(|trace| trace.queries.iter())
            .map(|query| query.stats.cache_hits as u64)
            .sum(),
    );
    for (counter, query_name) in [
        ("query.executions.parsed_module", "parsed_module"),
        (
            "query.executions.loader_active_module_item_tree_fact",
            "loader_active_module_item_tree_fact",
        ),
        (
            "query.executions.module_declarations",
            "module_declarations",
        ),
        ("query.executions.provider_summary", "provider_summary"),
        (
            "query.executions.module_facade_facts",
            "module_facade_facts",
        ),
        (
            "query.executions.loader_public_surface_module_facts",
            "loader_public_surface_module_facts",
        ),
        ("query.executions.module_defs", "module_defs"),
        ("query.executions.full_module_defs", "full_module_defs"),
        (
            "query.executions.public_surface_module_facts",
            "public_surface_module_facts",
        ),
        ("query.executions.module_item_tree", "module_item_tree"),
        (
            "query.executions.active_module_item_tree",
            "active_module_item_tree",
        ),
        ("query.executions.type_resolution", "type_resolution"),
        ("query.executions.type_lowering", "type_lowering"),
        ("query.executions.item_signatures", "item_signatures"),
        (
            "query.executions.signature_type_resolution",
            "signature_type_resolution",
        ),
        (
            "query.executions.signature_type_lowering",
            "signature_type_lowering",
        ),
        (
            "query.executions.signature_item_signatures",
            "signature_item_signatures",
        ),
        (
            "query.executions.signature_type_normalization",
            "signature_type_normalization",
        ),
        (
            "query.executions.module_program_signature_facts",
            "module_program_signature_facts",
        ),
        (
            "query.executions.extension_signature_module_input",
            "extension_signature_module_input",
        ),
        (
            "query.executions.extension_trait_solving_module_facts",
            "extension_trait_solving_module_facts",
        ),
        (
            "query.executions.extension_provider_module_facts",
            "extension_provider_module_facts",
        ),
        (
            "query.executions.extension_provider_nominal_module_facts",
            "extension_provider_nominal_module_facts",
        ),
        (
            "query.executions.extension_provider_validation_facts",
            "extension_provider_validation_facts",
        ),
        ("query.executions.const_module", "const_module"),
        ("query.executions.const", "const"),
        (
            "query.executions.const_array_lengths",
            "const_array_lengths",
        ),
        ("query.executions.const_enum_values", "const_enum_values"),
        ("query.executions.const_values", "const_values"),
        ("query.executions.const_typed_facts", "const_typed_facts"),
        ("query.executions.layouts", "layouts"),
        ("query.executions.abi_check", "abi_check"),
        ("query.executions.static_check", "static_check"),
        ("query.executions.flow_check", "flow_check"),
        ("query.executions.body_check", "body_check"),
        (
            "query.executions.executable_checked_module_facts",
            "executable_checked_module_facts",
        ),
        (
            "query.executions.executable_checked_modules",
            "executable_checked_modules",
        ),
        (
            "query.executions.executable_value_ref_item",
            "executable_value_ref_item",
        ),
        (
            "query.executions.executable_value_ref_edges",
            "executable_value_ref_edges",
        ),
        (
            "query.executions.executable_function_body",
            "executable_function_body",
        ),
        (
            "query.executions.executable_static_init",
            "executable_static_init",
        ),
        (
            "query.executions.lowered_function_body",
            "lowered_function_body",
        ),
        ("query.executions.signature_layouts", "signature_layouts"),
        ("query.executions.type_normalization", "type_normalization"),
    ] {
        nia_timing::emit_counter(
            counter,
            traces
                .iter()
                .flat_map(|trace| trace.queries.iter())
                .filter(|query| query.frame.name == query_name)
                .map(|query| query.stats.executions as u64)
                .sum(),
        );
    }
    nia_timing::emit_counter("driver.provider_demand_rounds", provider_demand_rounds);
    nia_timing::emit_counter(
        "compiler.checked_bodies",
        output.checked_body_count() as u64,
    );
    nia_timing::emit_counter(
        "compiler.reachable_bodies",
        output.reachable_body_count() as u64,
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoaderKey {
    entry_path: SourcePath,
    module_map: ModuleMap,
    target: TargetConfig,
    entry_runtime: EntryRuntime,
}

#[derive(Clone)]
struct SessionLoader {
    key: LoaderKey,
    database: LoaderDatabase,
}

impl fmt::Debug for SessionLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLoader")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct SessionCompiler {
    database: CompilerDatabase,
}

impl fmt::Debug for SessionCompiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionCompiler").finish_non_exhaustive()
    }
}

impl CheckRequest {
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self::from_source_path(SourcePath::new(entry_path.into()))
    }

    pub fn from_source_path(entry_path: SourcePath) -> Self {
        Self {
            entry_path,
            module_map: ModuleMap::default(),
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            runtime: Runtime::Bare,
        }
    }

    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    pub fn with_runtime(mut self, runtime: Runtime) -> Self {
        self.runtime = runtime;
        self
    }
}

fn entry_runtime(runtime: Runtime) -> EntryRuntime {
    match runtime {
        Runtime::Bare => EntryRuntime::None,
        Runtime::Freestanding => EntryRuntime::Freestanding,
    }
}

#[derive(Debug, Clone)]
pub struct EmitLlvmRequest {
    pub check: CheckRequest,
}

impl EmitLlvmRequest {
    pub fn new(check: CheckRequest) -> Self {
        Self { check }
    }
}

#[derive(Debug, Clone)]
pub struct EmitObjectRequest {
    pub check: CheckRequest,
}

impl EmitObjectRequest {
    pub fn new(check: CheckRequest) -> Self {
        Self { check }
    }
}

#[derive(Debug, Clone)]
pub struct WriteObjectRequest {
    pub check: CheckRequest,
    pub output: ObjectOutput,
}

impl WriteObjectRequest {
    pub fn new(check: CheckRequest, output: ObjectOutput) -> Self {
        Self { check, output }
    }
}

#[derive(Debug, Clone)]
pub struct LinkExecutableRequest {
    pub check: CheckRequest,
    pub output: PathBuf,
    pub link_options: LinkOptions,
}

impl LinkExecutableRequest {
    pub fn new(check: CheckRequest, output: impl Into<PathBuf>) -> Self {
        Self {
            check,
            output: output.into(),
            link_options: LinkOptions::default(),
        }
    }

    pub fn with_link_options(mut self, link_options: LinkOptions) -> Self {
        self.link_options = link_options;
        self
    }
}

#[derive(Debug, Clone)]
pub enum ObjectOutput {
    Single(PathBuf),
    Directory(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmIrArtifact {
    pub modules: Vec<nia_codegen_llvm::LlvmModuleOutput>,
    pub optimization: OptimizationPolicy,
    pub optimization_report: crate::BackendOptimizationReport,
    pub diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectArtifact {
    pub link_inputs: nia_codegen_llvm::IncrementalLinkInputs<nia_codegen_llvm::NativeObject>,
    pub optimization: OptimizationPolicy,
    pub optimization_report: crate::BackendOptimizationReport,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
struct ObjectArtifactWithSourceManifest {
    artifact: ObjectArtifact,
    source_manifest: SourceInputManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenObjectArtifact {
    pub link_inputs: nia_codegen_llvm::IncrementalLinkInputs<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutableArtifact {
    pub path: PathBuf,
    pub optimization: OptimizationPolicy,
    pub optimization_report: crate::BackendOptimizationReport,
    pub diagnostics: Vec<ProgramDiagnostic>,
    pub cache_reference: Option<ExecutableCacheReference>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticArchiveArtifact {
    pub path: PathBuf,
    pub optimization: OptimizationPolicy,
    pub optimization_report: crate::BackendOptimizationReport,
    pub diagnostics: Vec<ProgramDiagnostic>,
    pub cache_reference: Option<StaticArchiveCacheReference>,
}

#[derive(Debug)]
pub struct DriverOutput<T> {
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

    pub(crate) fn catch_ice(f: impl FnOnce() -> Self) -> Self {
        match nia_ice::catch_ice(f) {
            Ok(output) => output,
            Err(ice) => Self::from_error(DriverError::InternalDiagnostic(ice.diagnostic())),
        }
    }
}

#[derive(Debug)]
pub enum DriverError {
    ArchiveStatus {
        program: String,
        status: std::process::ExitStatus,
    },
    ArchiveIo {
        program: String,
        error: io::Error,
    },
    ArchiveConfig(nia_linker::LinkerConfigError),
    CheckDiagnostics(CheckedProgram),
    CodegenProgramDiagnostics(Box<CodegenProgram>),
    CodegenPreparationDiagnostics(Vec<nia_compiler_query::ProgramDiagnostic>),
    CodegenDiagnostics(Vec<Diagnostic>),
    InternalDiagnostic(Diagnostic),
    InvalidArtifactRequest(String),
    Io {
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
    },
    LinkerStatus {
        program: String,
        status: std::process::ExitStatus,
    },
    LinkerIo {
        program: String,
        error: io::Error,
    },
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

fn write_output_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn object_file_name(index: usize, module_name: &str) -> String {
    let stem = Path::new(module_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let clean = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{index:04}_{clean}.o")
}

fn archive_member_file_name(index: usize, key: &nia_codegen_llvm::CodegenUnitKey) -> String {
    let stable_name = match key {
        nia_codegen_llvm::CodegenUnitKey::SourceModule {
            source_identity, ..
        } => source_identity.normalized_path(),
        nia_codegen_llvm::CodegenUnitKey::CompilerBuiltins => "nia_compiler_builtins",
    };
    object_file_name(index, stable_name)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = env::temp_dir();
        path.push(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
