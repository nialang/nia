// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

use nia_compiler_query::{CompileRequest, CompilerDatabase, TimingMode};
use nia_diagnostic::Diagnostic;
use nia_imports::ModuleMap;
use nia_linker::{LinkOptions, LinkTarget};
use nia_loader_query::{EntryRuntime, LoadRequest, LoaderDatabase};
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_source::{SourceDatabase, SourcePath};
use nia_target_config::TargetConfig;

use crate::{CheckedProgram, LoadedProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Bare,
    Freestanding,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::Bare
    }
}

#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub entry_path: String,
    pub module_map: ModuleMap,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
    pub runtime: Runtime,
}

#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub target: TargetConfig,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            target: TargetConfig::host(),
        }
    }
}

#[derive(Debug)]
pub struct Driver {
    config: DriverConfig,
    sources: SourceDatabase,
    loader: std::sync::Arc<Mutex<Option<SessionLoader>>>,
    compiler: std::sync::Arc<Mutex<Option<SessionCompiler>>>,
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver {
    pub fn new() -> Self {
        Self::with_config(DriverConfig::default())
    }

    pub fn with_config(config: DriverConfig) -> Self {
        Self {
            config,
            sources: SourceDatabase::new(),
            loader: std::sync::Arc::new(Mutex::new(None)),
            compiler: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    pub fn config(&self) -> &DriverConfig {
        &self.config
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn set_source(&self, path: impl Into<String>, text: impl Into<String>) {
        let path = path.into();
        let loader = self.loader.lock().expect("driver loader lock poisoned");
        if let Some(loader) = &*loader {
            loader.database.set_source(path, text);
        } else {
            drop(loader);
            self.sources.set_source(SourcePath::new(path), text);
        }
    }

    pub fn check(&self, request: CheckRequest) -> DriverOutput<CheckedProgram> {
        DriverOutput::catch_ice(|| {
            let program = self.check_inner(request);
            if !program.diagnostics.is_empty() {
                return DriverOutput::from_check_diagnostics(program);
            }
            DriverOutput::success(program)
        })
    }

    fn check_inner(&self, request: CheckRequest) -> CheckedProgram {
        let loaded = self.load_program(&request);
        let compile_request = CompileRequest::new(loaded)
            .with_optimization(request.optimization)
            .with_timings(request.timings);
        let mut compiler_guard = self.compiler.lock().expect("driver compiler lock poisoned");
        let database = match &*compiler_guard {
            Some(compiler) => {
                compiler.database.update(compile_request);
                compiler.database.clone()
            }
            _ => {
                let database = CompilerDatabase::new(compile_request);
                *compiler_guard = Some(SessionCompiler {
                    database: database.clone(),
                });
                database
            }
        };
        let checked = database.check_program();
        drop(compiler_guard);
        checked
    }

    pub fn emit_llvm_ir(&self, request: EmitLlvmRequest) -> DriverOutput<LlvmIrArtifact> {
        DriverOutput::catch_ice(|| {
            let program = match self.check(request.check).result {
                Ok(program) => program,
                Err(error) => return DriverOutput::from_error(error),
            };
            self.emit_llvm_ir_from_checked(&program)
        })
    }

    pub fn emit_llvm_ir_from_checked(
        &self,
        program: &CheckedProgram,
    ) -> DriverOutput<LlvmIrArtifact> {
        DriverOutput::catch_ice(|| {
            let output = nia_codegen_llvm::emit_llvm_ir_with_options(
                &program.backend_lowering.program,
                codegen_options(program.optimization),
            );
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            DriverOutput::success(LlvmIrArtifact {
                modules: output.modules,
            })
        })
    }

    pub fn emit_native_objects(&self, request: EmitObjectRequest) -> DriverOutput<ObjectArtifact> {
        DriverOutput::catch_ice(|| {
            let program = match self.check(request.check).result {
                Ok(program) => program,
                Err(error) => return DriverOutput::from_error(error),
            };
            self.emit_native_objects_from_checked(&program)
        })
    }

    pub fn emit_native_objects_from_checked(
        &self,
        program: &CheckedProgram,
    ) -> DriverOutput<ObjectArtifact> {
        DriverOutput::catch_ice(|| {
            let output = nia_codegen_llvm::emit_native_objects(
                &program.backend_lowering.program,
                codegen_options(program.optimization),
            );
            if !output.diagnostics.is_empty() {
                return DriverOutput::from_error(DriverError::CodegenDiagnostics(
                    output.diagnostics,
                ));
            }
            DriverOutput::success(ObjectArtifact {
                modules: output.modules,
            })
        })
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
                    if objects.modules.len() != 1 {
                        return DriverOutput::from_error(DriverError::InvalidArtifactRequest(
                            "`-o` can only be used when the program has one codegen unit; use `--out-dir`"
                                .to_string(),
                        ));
                    }
                    if let Err(error) = write_output_file(&path, &objects.modules[0].bytes) {
                        return DriverOutput::from_error(DriverError::Io {
                            path,
                            operation: "write object file",
                            error,
                        });
                    }
                    vec![path]
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
                    for (index, module) in objects.modules.iter().enumerate() {
                        let path = dir.join(object_file_name(index, &module.name));
                        if let Err(error) = write_output_file(&path, &module.bytes) {
                            return DriverOutput::from_error(DriverError::Io {
                                path,
                                operation: "write object file",
                                error,
                            });
                        }
                        paths.push(path);
                    }
                    paths
                }
            };
            DriverOutput::success(WrittenObjectArtifact { paths: written })
        })
    }

    pub fn link_executable(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<ExecutableArtifact> {
        DriverOutput::catch_ice(|| {
            let mut request = request;
            request.link_options.target = LinkTarget::from_target_config(&self.config.target);
            let output = self.emit_native_objects(EmitObjectRequest {
                check: request.check.with_runtime(Runtime::Freestanding),
            });
            let objects = match output.result {
                Ok(objects) => objects,
                Err(error) => return DriverOutput::from_error(error),
            };
            self.link_executable_from_objects(&objects, request.output, request.link_options)
        })
    }

    pub fn link_executable_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        mut link_options: LinkOptions,
    ) -> DriverOutput<ExecutableArtifact> {
        DriverOutput::catch_ice(|| {
            link_options.target = LinkTarget::from_target_config(&self.config.target);
            let temp = TempDir::new("nia_emit_exe");
            if let Err(error) = fs::create_dir_all(temp.path()) {
                return DriverOutput::from_error(DriverError::Io {
                    path: temp.path().to_path_buf(),
                    operation: "create temporary object directory",
                    error,
                });
            }
            let mut object_paths = Vec::new();
            for (index, module) in objects.modules.iter().enumerate() {
                let object_path = temp.path().join(object_file_name(index, &module.name));
                if let Err(error) = write_output_file(&object_path, &module.bytes) {
                    return DriverOutput::from_error(DriverError::Io {
                        path: object_path,
                        operation: "write temporary object file",
                        error,
                    });
                }
                object_paths.push(object_path);
            }
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

            let invocation = match link_options.invocation(&object_paths, output.clone()) {
                Ok(invocation) => invocation,
                Err(error) => return DriverOutput::from_error(DriverError::LinkerConfig(error)),
            };
            match Command::new(&invocation.program)
                .args(&invocation.args)
                .status()
            {
                Ok(status) if status.success() => {
                    DriverOutput::success(ExecutableArtifact { path: output })
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

    fn load_program(&self, request: &CheckRequest) -> LoadedProgram {
        let key = LoaderKey {
            entry_path: request.entry_path.clone(),
            module_map: request.module_map.clone(),
            target: self.config.target.clone(),
            entry_runtime: entry_runtime(request.runtime),
        };
        let mut loader_guard = self.loader.lock().expect("driver loader lock poisoned");
        let database = match &*loader_guard {
            Some(loader) if loader.key == key => loader.database.clone(),
            _ => {
                let database = LoaderDatabase::new(
                    LoadRequest::new(key.entry_path.clone())
                        .with_module_map(key.module_map.clone())
                        .with_sources(self.sources.clone())
                        .with_target(key.target.clone())
                        .with_entry_runtime(key.entry_runtime),
                );
                *loader_guard = Some(SessionLoader {
                    key,
                    database: database.clone(),
                });
                database
            }
        };
        drop(loader_guard);
        database.load_program()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoaderKey {
    entry_path: String,
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
        Self {
            entry_path: entry_path.into(),
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
}

#[derive(Debug, Clone)]
pub enum ObjectOutput {
    Single(PathBuf),
    Directory(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmIrArtifact {
    pub modules: Vec<nia_codegen_llvm::LlvmModuleOutput>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectArtifact {
    pub modules: Vec<nia_codegen_llvm::LlvmObjectModuleOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenObjectArtifact {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableArtifact {
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct DriverOutput<T> {
    pub result: Result<T, DriverError>,
}

impl<T> DriverOutput<T> {
    fn success(value: T) -> Self {
        Self { result: Ok(value) }
    }

    fn from_error(error: DriverError) -> Self {
        Self { result: Err(error) }
    }

    fn from_check_diagnostics(program: CheckedProgram) -> Self {
        Self::from_error(DriverError::CheckDiagnostics(program))
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
    CheckDiagnostics(CheckedProgram),
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

fn codegen_options(optimization: OptimizationPolicy) -> nia_codegen_llvm::LlvmCodegenOptions {
    nia_codegen_llvm::LlvmCodegenOptions { optimization }
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
