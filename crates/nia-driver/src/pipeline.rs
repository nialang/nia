// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

use nia_compiler_query::{CompileRequest, CompilerDatabase, TimingMode};
use nia_diagnostic::Diagnostic;
use nia_ids::ModuleId;
use nia_imports::{ModuleMap, ModulePath};
use nia_loader_query::{EntryRuntime, LoadRequest, LoaderDatabase};
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_source::{SourceDatabase, SourcePath, SourceVersion};
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
    pub root_path: String,
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

    pub fn check(&self, request: CheckRequest) -> CheckedProgram {
        let _permit = check_test_permit();
        let loaded = self.load_program(&request);
        let key = CompilerKey::new(&loaded, request.optimization, request.timings);
        let mut compiler_guard = self.compiler.lock().expect("driver compiler lock poisoned");
        let database = match &*compiler_guard {
            Some(compiler) if compiler.key == key => compiler.database.clone(),
            _ => {
                let database = CompilerDatabase::new(
                    CompileRequest::new(loaded)
                        .with_optimization(request.optimization)
                        .with_timings(request.timings),
                );
                *compiler_guard = Some(SessionCompiler {
                    key,
                    database: database.clone(),
                });
                database
            }
        };
        drop(compiler_guard);
        database.check_program()
    }

    pub fn emit_llvm_ir(&self, request: EmitLlvmRequest) -> DriverOutput<LlvmIrArtifact> {
        let program = self.check(request.check);
        if !program.diagnostics.is_empty() {
            return DriverOutput::from_check_diagnostics(program);
        }
        self.emit_llvm_ir_from_checked(&program)
    }

    pub fn emit_llvm_ir_from_checked(
        &self,
        program: &CheckedProgram,
    ) -> DriverOutput<LlvmIrArtifact> {
        let output = nia_codegen_llvm::emit_llvm_ir_with_options(
            &program.backend_lowering.program,
            codegen_options(program.optimization),
        );
        if !output.diagnostics.is_empty() {
            return DriverOutput::from_error(DriverError::CodegenDiagnostics(output.diagnostics));
        }
        DriverOutput::success(LlvmIrArtifact {
            modules: output.modules,
        })
    }

    pub fn emit_native_objects(&self, request: EmitObjectRequest) -> DriverOutput<ObjectArtifact> {
        let program = self.check(request.check);
        if !program.diagnostics.is_empty() {
            return DriverOutput::from_check_diagnostics(program);
        }
        self.emit_native_objects_from_checked(&program)
    }

    pub fn emit_native_objects_from_checked(
        &self,
        program: &CheckedProgram,
    ) -> DriverOutput<ObjectArtifact> {
        let output = nia_codegen_llvm::emit_native_objects(
            &program.backend_lowering.program,
            codegen_options(program.optimization),
        );
        if !output.diagnostics.is_empty() {
            return DriverOutput::from_error(DriverError::CodegenDiagnostics(output.diagnostics));
        }
        DriverOutput::success(ObjectArtifact {
            modules: output.modules,
        })
    }

    pub fn write_native_objects(
        &self,
        request: WriteObjectRequest,
    ) -> DriverOutput<WrittenObjectArtifact> {
        let output = self.emit_native_objects(EmitObjectRequest {
            check: request.check,
        });
        let objects = match output.result {
            Ok(objects) => objects,
            Err(error) => return DriverOutput::from_error(error),
        };
        self.write_native_objects_from_artifact(&objects, request.output)
    }

    pub fn write_native_objects_from_artifact(
        &self,
        objects: &ObjectArtifact,
        output: ObjectOutput,
    ) -> DriverOutput<WrittenObjectArtifact> {
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
    }

    pub fn link_executable(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<ExecutableArtifact> {
        let output = self.emit_native_objects(EmitObjectRequest {
            check: request.check.with_runtime(Runtime::Freestanding),
        });
        let objects = match output.result {
            Ok(objects) => objects,
            Err(error) => return DriverOutput::from_error(error),
        };
        self.link_executable_from_objects(
            &objects,
            request.output,
            request.link_args,
            request.linker.unwrap_or_else(ExecutableLinker::native),
        )
    }

    pub fn link_executable_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        link_args: Vec<String>,
        linker: ExecutableLinker,
    ) -> DriverOutput<ExecutableArtifact> {
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

        match Command::new(&linker.program)
            .args(&linker.args_before_objects)
            .args(&object_paths)
            .args(&linker.args_after_objects)
            .args(&link_args)
            .arg("-o")
            .arg(&output)
            .status()
        {
            Ok(status) if status.success() => {
                DriverOutput::success(ExecutableArtifact { path: output })
            }
            Ok(status) => DriverOutput::from_error(DriverError::LinkerStatus {
                program: linker.program,
                status,
            }),
            Err(error) => DriverOutput::from_error(DriverError::LinkerIo {
                program: linker.program,
                error,
            }),
        }
    }

    fn load_program(&self, request: &CheckRequest) -> LoadedProgram {
        let key = LoaderKey {
            root_path: request.root_path.clone(),
            module_map: request.module_map.clone(),
            target: self.config.target.clone(),
            entry_runtime: entry_runtime(request.runtime),
        };
        let mut loader_guard = self.loader.lock().expect("driver loader lock poisoned");
        let database = match &*loader_guard {
            Some(loader) if loader.key == key => loader.database.clone(),
            _ => {
                let database = LoaderDatabase::new(
                    LoadRequest::new(key.root_path.clone())
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
    root_path: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerKey {
    graph: CompilerGraphKey,
    target: TargetConfig,
    runtime: nia_compiler_query::RuntimeModel,
    optimization: NiaOptimizationLevel,
    timings: TimingMode,
    modules: Vec<CompilerModuleKey>,
}

impl CompilerKey {
    fn new(
        loaded: &LoadedProgram,
        optimization: NiaOptimizationLevel,
        timings: TimingMode,
    ) -> Self {
        Self {
            graph: CompilerGraphKey::new(&loaded.graph),
            target: loaded.target.clone(),
            runtime: loaded.runtime,
            optimization,
            timings,
            modules: loaded
                .modules
                .iter()
                .map(|module| CompilerModuleKey {
                    id: module.id,
                    path: module.path.clone(),
                    source_version: module.source_version,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerGraphKey {
    root: ModuleId,
    modules: Vec<CompilerGraphModuleKey>,
    diagnostics_len: usize,
}

impl CompilerGraphKey {
    fn new(graph: &nia_imports::ModuleGraph) -> Self {
        Self {
            root: graph.root(),
            modules: graph
                .modules()
                .map(|module| CompilerGraphModuleKey {
                    id: module.id,
                    path: module.path.clone(),
                    module_path: module.module_path.clone(),
                    parent: module.parent,
                    children: sorted_children(&module.children),
                    declarations: module
                        .declarations
                        .iter()
                        .map(|declaration| CompilerGraphDeclarationKey {
                            name: declaration.name.clone(),
                            visibility: declaration.visibility,
                            target: declaration.target,
                        })
                        .collect(),
                })
                .collect(),
            diagnostics_len: graph.diagnostics().len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerGraphModuleKey {
    id: ModuleId,
    path: SourcePath,
    module_path: ModulePath,
    parent: Option<ModuleId>,
    children: Vec<(String, ModuleId)>,
    declarations: Vec<CompilerGraphDeclarationKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerGraphDeclarationKey {
    name: String,
    visibility: nia_ast::Visibility,
    target: ModuleId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerModuleKey {
    id: ModuleId,
    path: SourcePath,
    source_version: SourceVersion,
}

#[derive(Clone)]
struct SessionCompiler {
    key: CompilerKey,
    database: CompilerDatabase,
}

impl fmt::Debug for SessionCompiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionCompiler")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

fn sorted_children(
    children: &std::collections::HashMap<String, ModuleId>,
) -> Vec<(String, ModuleId)> {
    let mut children = children
        .iter()
        .map(|(name, module_id)| (name.clone(), *module_id))
        .collect::<Vec<_>>();
    children.sort_by(|left, right| left.0.cmp(&right.0));
    children
}

impl CheckRequest {
    pub fn new(root_path: impl Into<String>) -> Self {
        Self {
            root_path: root_path.into(),
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
    pub link_args: Vec<String>,
    pub linker: Option<ExecutableLinker>,
}

impl LinkExecutableRequest {
    pub fn new(check: CheckRequest, output: impl Into<PathBuf>) -> Self {
        Self {
            check,
            output: output.into(),
            link_args: Vec::new(),
            linker: None,
        }
    }

    pub fn with_link_args(mut self, link_args: Vec<String>) -> Self {
        self.link_args = link_args;
        self
    }

    pub fn with_linker(mut self, linker: ExecutableLinker) -> Self {
        self.linker = Some(linker);
        self
    }
}

#[derive(Debug, Clone)]
pub enum ObjectOutput {
    Single(PathBuf),
    Directory(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ExecutableLinker {
    pub program: String,
    pub args_before_objects: Vec<String>,
    pub args_after_objects: Vec<String>,
}

impl ExecutableLinker {
    pub fn native() -> Self {
        if let Ok(program) = env::var("NIA_LINKER")
            && !program.is_empty()
        {
            return Self::with_program(program);
        }
        Self::with_program("ld")
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args_before_objects: Vec::new(),
            args_after_objects: vec!["-e".to_string(), "_start".to_string()],
        }
    }
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
}

#[derive(Debug)]
pub enum DriverError {
    CheckDiagnostics(CheckedProgram),
    CodegenDiagnostics(Vec<Diagnostic>),
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

#[cfg(test)]
fn check_test_permit() -> CheckTestPermit {
    const MAX_CHECKS: usize = 4;

    let (running, available) = check_test_limit();
    let mut count = running
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *count >= MAX_CHECKS {
        count = available
            .wait(count)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    *count += 1;
    CheckTestPermit
}

#[cfg(not(test))]
fn check_test_permit() -> CheckTestPermit {
    CheckTestPermit
}

struct CheckTestPermit;

#[cfg(test)]
impl Drop for CheckTestPermit {
    fn drop(&mut self) {
        let (running, available) = check_test_limit();
        let mut count = running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *count -= 1;
        available.notify_one();
    }
}

#[cfg(test)]
fn check_test_limit() -> &'static (std::sync::Mutex<usize>, std::sync::Condvar) {
    use std::sync::{Condvar, Mutex, OnceLock};

    static LIMIT: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    LIMIT.get_or_init(|| (Mutex::new(0), Condvar::new()))
}
