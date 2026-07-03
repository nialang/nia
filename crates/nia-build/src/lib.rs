// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nia_driver::{CheckRequest, Driver, DriverError, LinkExecutableRequest, TimingMode};
use nia_imports::ModuleMap;
use nia_source::SourcePath;

const BUILD_RUNNER_FINGERPRINT_VERSION: &str = "nia-build-runner-fingerprint-v3";
const BUILD_RUNNER_MANIFEST_VERSION: &str = "nia-build-runner-manifest-v1";
const BUILD_RUNNER_TOOLCHAIN_ABI_VERSION: &str = "nia-build-runner-toolchain-abi-v1";
static SHARED_RUNNER_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub root: Option<PathBuf>,
    pub step: Option<String>,
    pub timings: TimingMode,
}

impl BuildRequest {
    pub fn new() -> Self {
        Self {
            root: None,
            step: None,
            timings: TimingMode::Off,
        }
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn with_step(mut self, step: impl Into<String>) -> Self {
        self.step = Some(step.into());
        self
    }

    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }
}

impl Default for BuildRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub package_root: PathBuf,
    pub build_script: PathBuf,
    pub toolchain_executable: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runner_dir: PathBuf,
    pub runner_executable: PathBuf,
    pub step: BuildStepSelection,
    pub timings: TimingMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStepSelection {
    Default,
    Named(String),
}

impl BuildStepSelection {
    fn as_runner_arg(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Named(step) => Some(step),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRunnerSource {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildRunnerFingerprintSnapshot {
    fingerprint: String,
    runner_source_hash: String,
    inputs: Vec<BuildRunnerFingerprintInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildRunnerFingerprintInput {
    root: BuildRunnerFingerprintRoot,
    relative_path: PathBuf,
    content_len: u64,
    content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildRunnerFingerprintRoot {
    Build,
    Std,
}

impl BuildRunnerFingerprintRoot {
    fn as_manifest_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Std => "std",
        }
    }

    fn from_manifest_str(text: &str) -> Option<Self> {
        match text {
            "build" => Some(Self::Build),
            "std" => Some(Self::Std),
            _ => None,
        }
    }

    fn root_dir(self, plan: &BuildPlan) -> PathBuf {
        match self {
            Self::Build => plan.package_root.clone(),
            Self::Std => workspace_std_root(),
        }
    }
}

#[derive(Debug)]
pub enum BuildError {
    CurrentDirectory {
        error: io::Error,
    },
    CurrentExecutable {
        error: io::Error,
    },
    CreateBuildDirectory {
        path: PathBuf,
        error: io::Error,
    },
    CreateCacheDirectory {
        path: PathBuf,
        error: io::Error,
    },
    NonUtf8Path {
        role: &'static str,
        path: PathBuf,
    },
    CreateRunnerDirectory {
        path: PathBuf,
        error: io::Error,
    },
    CompileRunner {
        path: String,
        source: String,
        error: DriverError,
    },
    BuildRunnerFingerprint {
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
    },
    RunRunner {
        path: PathBuf,
        error: io::Error,
    },
    AcquireBuildLock {
        path: PathBuf,
        error: io::Error,
    },
    RunnerFailed {
        path: PathBuf,
        status: ExitStatus,
    },
    MissingBuildScript {
        start: PathBuf,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { error } => {
                write!(f, "failed to read current directory: {error}")
            }
            Self::CurrentExecutable { error } => {
                write!(
                    f,
                    "failed to read current toolchain executable path: {error}"
                )
            }
            Self::CreateBuildDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build directory `{}`: {error}",
                    path.display()
                )
            }
            Self::CreateCacheDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build cache directory `{}`: {error}",
                    path.display()
                )
            }
            Self::NonUtf8Path { role, path } => {
                write!(
                    f,
                    "failed to encode {role} path `{}` as Nia source: path is not valid UTF-8",
                    path.display()
                )
            }
            Self::CreateRunnerDirectory { path, error } => {
                write!(
                    f,
                    "failed to create build runner directory `{}`: {error}",
                    path.display()
                )
            }
            Self::CompileRunner {
                path,
                source,
                error,
            } => {
                write!(
                    f,
                    "failed to compile build runner `{path}`\n{}",
                    nia_driver::render_driver_error(error, Some(path), Some(source))
                )
            }
            Self::BuildRunnerFingerprint {
                path,
                operation,
                error,
            } => {
                write!(
                    f,
                    "failed to {operation} build runner fingerprint `{}`: {error}",
                    path.display()
                )
            }
            Self::RunRunner { path, error } => {
                write!(
                    f,
                    "failed to run build runner `{}`: {error}",
                    path.display()
                )
            }
            Self::AcquireBuildLock { path, error } => {
                write!(
                    f,
                    "failed to acquire build lock `{}`: {error}",
                    path.display()
                )
            }
            Self::RunnerFailed { path, status } => {
                write!(
                    f,
                    "build runner `{}` exited with status {status}",
                    path.display()
                )
            }
            Self::MissingBuildScript { start } => write!(
                f,
                "failed to find `build.nia` from `{}` or any parent directory",
                start.display()
            ),
        }
    }
}

impl std::error::Error for BuildError {}

pub fn run_build(request: BuildRequest) -> Result<(), BuildError> {
    let timings = request.timings;
    let plan = time_stage(timings, "build_resolve_plan", || {
        resolve_build_plan(request)
    })?;
    time_stage(timings, "build_prepare_directories", || {
        prepare_build_directories(&plan)
    })?;
    let runner_executable = time_stage(timings, "build_compile_runner", || {
        compile_build_runner(&plan)
    })?;
    let _lock = time_stage(timings, "build_acquire_lock", || BuildLock::acquire(&plan))?;
    time_stage(timings, "build_run_runner", || {
        run_build_runner(&plan, &runner_executable)
    })
}

pub fn resolve_build_plan(request: BuildRequest) -> Result<BuildPlan, BuildError> {
    let start = match request.root {
        Some(root) => root,
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let toolchain_executable =
        env::current_exe().map_err(|error| BuildError::CurrentExecutable { error })?;
    let package_root = find_package_root(&start)?;
    let build_script = package_root.join("build.nia");
    let build_dir = package_root.join(".nia-build");
    let runner_dir = build_dir.join("runner");
    Ok(BuildPlan {
        cache_dir: package_root.join(".nia-cache"),
        runner_executable: runner_dir.join("nia-build-runner"),
        runner_dir,
        build_dir,
        toolchain_executable,
        package_root,
        build_script,
        step: request
            .step
            .map(BuildStepSelection::Named)
            .unwrap_or(BuildStepSelection::Default),
        timings: request.timings,
    })
}

pub fn build_runner_source(plan: &BuildPlan) -> Result<BuildRunnerSource, BuildError> {
    let path = plan
        .runner_dir
        .join("root.nia")
        .to_string_lossy()
        .into_owned();
    build_runner_source_for_path(plan, path)
}

fn build_runner_source_for_path(
    plan: &BuildPlan,
    path: String,
) -> Result<BuildRunnerSource, BuildError> {
    let _ = plan;
    let mut source = String::new();
    source.push_str(
        r#"
using std::build;
using std::collections;
using std::fs;
using std::mem;
using std::process;
using build_script;

fn path_arg(
    init: process::Init,
    allocator: &mut mem::Allocator,
    index: usize,
    storage: &mut collections::ArrayList[char],
) build::Error!fs::PathView {
    let arg = if ?value = init.args().get(index) {
        value
    } or null {
        return build::Error::Internal!;
    };
    fs::PathView::from_utf8_into(allocator, arg.bytes(), storage).as_build_error()
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut page_allocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut page_allocator);
    defer allocator.deinit().ok().exit().?;

    let mut package_root_text = collections::ArrayList[char]::init();
    defer package_root_text.deinit(&mut allocator).exit().?;
    let mut build_dir_text = collections::ArrayList[char]::init();
    defer build_dir_text.deinit(&mut allocator).exit().?;
    let mut cache_dir_text = collections::ArrayList[char]::init();
    defer cache_dir_text.deinit(&mut allocator).exit().?;
    let mut toolchain_text = collections::ArrayList[char]::init();
    defer toolchain_text.deinit(&mut allocator).exit().?;

    let package_root = path_arg(init, &mut allocator, 1usize, &mut package_root_text).exit().?;
    let build_dir = path_arg(init, &mut allocator, 2usize, &mut build_dir_text).exit().?;
    let cache_dir = path_arg(init, &mut allocator, 3usize, &mut cache_dir_text).exit().?;
    let toolchain_executable = path_arg(init, &mut allocator, 4usize, &mut toolchain_text).exit().?;

    let mut api = build::Build::init(
        init,
        &mut allocator,
"#,
    );
    source.push_str(
        r#"        package_root,
        build_dir,
        cache_dir,
        toolchain_executable,
        5usize,
"#,
    );
    source.push_str(
        r#"    );
    defer api.deinit().exit().?;

    build_script::build(&mut api).exit().?;
    api.run_requested_step().exit().?;
    !{}
}
"#,
    );
    Ok(BuildRunnerSource { path, source })
}

fn compile_build_runner(plan: &BuildPlan) -> Result<PathBuf, BuildError> {
    fs::create_dir_all(&plan.runner_dir).map_err(|error| BuildError::CreateRunnerDirectory {
        path: plan.runner_dir.clone(),
        error,
    })?;
    let runner = build_runner_source(plan)?;
    if let Some(fingerprint) = restore_build_runner_fingerprint(plan, &runner)? {
        let local_runner = local_build_runner_executable(plan, &fingerprint);
        if build_runner_cache_valid(&local_runner) {
            return Ok(local_runner);
        }
        let shared_runner = shared_build_runner_executable(&fingerprint);
        if shared_runner.is_file() {
            install_shared_build_runner(&local_runner, &shared_runner)?;
            return Ok(local_runner);
        }
    }
    let snapshot = build_runner_fingerprint(plan, &runner)?;
    let fingerprint = snapshot.fingerprint.clone();
    let local_runner = local_build_runner_executable(plan, &fingerprint);
    if build_runner_cache_valid(&local_runner) {
        save_build_runner_manifest(plan, &snapshot)?;
        return Ok(local_runner);
    }
    let shared_runner = shared_build_runner_executable(&fingerprint);
    if shared_runner.is_file() {
        install_shared_build_runner(&local_runner, &shared_runner)?;
        save_build_runner_manifest(plan, &snapshot)?;
        return Ok(local_runner);
    }
    let _compile_lock = SharedRunnerCompileLock::acquire(&fingerprint)?;
    if build_runner_cache_valid(&local_runner) {
        save_build_runner_manifest(plan, &snapshot)?;
        return Ok(local_runner);
    }
    if shared_runner.is_file() {
        install_shared_build_runner(&local_runner, &shared_runner)?;
        save_build_runner_manifest(plan, &snapshot)?;
        return Ok(local_runner);
    }
    if let Some(parent) = local_runner.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let stage_id = SHARED_RUNNER_STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged_runner =
        local_runner.with_extension(format!("tmp.{}.{}", std::process::id(), stage_id));
    let driver = Driver::new();
    driver.set_source(runner.path.clone(), runner.source.clone());
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "build_script",
        SourcePath::new(plan.build_script.to_string_lossy().into_owned()),
    );
    let output = driver.link_executable(LinkExecutableRequest::new(
        CheckRequest::new(runner.path.clone())
            .with_module_map(module_map)
            .with_timings(plan.timings),
        &staged_runner,
    ));
    output
        .result
        .map(|_| ())
        .map_err(|error| BuildError::CompileRunner {
            path: runner.path,
            source: runner.source,
            error,
        })?;
    make_executable_if_needed(&staged_runner, &staged_runner)?;
    publish_runner(&staged_runner, &local_runner)?;
    save_shared_build_runner(&local_runner, &fingerprint)?;
    save_build_runner_manifest(plan, &snapshot)?;
    Ok(local_runner)
}

fn prepare_build_directories(plan: &BuildPlan) -> Result<(), BuildError> {
    fs::create_dir_all(&plan.build_dir).map_err(|error| BuildError::CreateBuildDirectory {
        path: plan.build_dir.clone(),
        error,
    })?;
    fs::create_dir_all(&plan.cache_dir).map_err(|error| BuildError::CreateCacheDirectory {
        path: plan.cache_dir.clone(),
        error,
    })
}

fn build_runner_fingerprint(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
) -> Result<BuildRunnerFingerprintSnapshot, BuildError> {
    let inputs = build_runner_fingerprint_inputs(plan, runner)?;
    Ok(BuildRunnerFingerprintSnapshot {
        fingerprint: build_runner_fingerprint_from_inputs(&runner.source, &inputs),
        runner_source_hash: content_hash(&runner.source),
        inputs,
    })
}

fn build_runner_fingerprint_from_inputs(
    runner_source: &str,
    inputs: &[BuildRunnerFingerprintInput],
) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(BUILD_RUNNER_FINGERPRINT_VERSION);
    hash.string(BUILD_RUNNER_TOOLCHAIN_ABI_VERSION);
    hash.string(env!("CARGO_PKG_VERSION"));
    hash.string(runner_source);
    for input in inputs {
        hash.string(input.root.as_manifest_str());
        hash.path(&input.relative_path);
        hash.string(&input.content_len.to_string());
        hash.string(&input.content_hash);
    }
    hash.finish()
}

fn build_runner_fingerprint_inputs(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
) -> Result<Vec<BuildRunnerFingerprintInput>, BuildError> {
    let mut inputs = Vec::new();
    inputs.extend(build_runner_build_package_inputs(plan, runner)?);
    inputs.extend(std_build_runner_inputs()?);
    inputs.sort_by(|lhs, rhs| {
        lhs.root
            .as_manifest_str()
            .cmp(rhs.root.as_manifest_str())
            .then_with(|| lhs.relative_path.cmp(&rhs.relative_path))
    });
    inputs.dedup_by(|lhs, rhs| lhs.root == rhs.root && lhs.relative_path == rhs.relative_path);
    Ok(inputs)
}

fn build_runner_build_package_inputs(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
) -> Result<Vec<BuildRunnerFingerprintInput>, BuildError> {
    let mut files = loaded_build_runner_module_files(plan, runner)?;
    files.sort();
    files.dedup();
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        inputs.push(build_runner_fingerprint_input(
            BuildRunnerFingerprintRoot::Build,
            &plan.package_root,
            &file,
        )?);
    }
    Ok(inputs)
}

fn std_build_runner_inputs() -> Result<Vec<BuildRunnerFingerprintInput>, BuildError> {
    let root = workspace_std_root();
    let mut files = Vec::new();
    collect_nia_files(&root, &mut files)?;
    files.sort();
    let mut inputs = Vec::with_capacity(files.len());
    for file in files {
        inputs.push(build_runner_fingerprint_input(
            BuildRunnerFingerprintRoot::Std,
            &root,
            &file,
        )?);
    }
    Ok(inputs)
}

fn build_runner_fingerprint_input(
    root_kind: BuildRunnerFingerprintRoot,
    root: &Path,
    file: &Path,
) -> Result<BuildRunnerFingerprintInput, BuildError> {
    let relative_path = safe_relative_build_runner_input_path(root, file).ok_or_else(|| {
        BuildError::BuildRunnerFingerprint {
            path: file.to_path_buf(),
            operation: "relativize",
            error: io::Error::new(
                io::ErrorKind::InvalidInput,
                "input path is outside fingerprint root",
            ),
        }
    })?;
    let text = fs::read_to_string(file).map_err(|error| BuildError::BuildRunnerFingerprint {
        path: file.to_path_buf(),
        operation: "read",
        error,
    })?;
    Ok(BuildRunnerFingerprintInput {
        root: root_kind,
        relative_path,
        content_len: text.len() as u64,
        content_hash: content_hash(&text),
    })
}

fn safe_relative_build_runner_input_path(root: &Path, file: &Path) -> Option<PathBuf> {
    let relative = file.strip_prefix(root).ok()?;
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(relative.to_path_buf())
}

fn loaded_build_runner_module_files(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
) -> Result<Vec<PathBuf>, BuildError> {
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "build_script",
        SourcePath::new(plan.build_script.to_string_lossy().into_owned()),
    );
    let sources = nia_source::SourceDatabase::new();
    let fingerprint_source = build_runner_fingerprint_source(runner);
    sources.set_source(SourcePath::new(runner.path.clone()), fingerprint_source);
    let loaded = nia_loader_query::load_program_request(
        nia_loader_query::LoadRequest::new(runner.path.clone())
            .with_module_map(module_map)
            .with_sources(sources),
    );

    let mut paths = loaded
        .modules
        .iter()
        .filter_map(|module| {
            let node = loaded.graph.get(module.id)?;
            (node.module_path.package == "build_script")
                .then(|| PathBuf::from(module.path.as_str()))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn build_runner_fingerprint_source(runner: &BuildRunnerSource) -> String {
    let mut source = runner.source.clone();
    source.push_str("\nusing build_script::*;\n");
    source
}

fn build_runner_cache_valid(runner: &Path) -> bool {
    runner.is_file()
}

fn restore_build_runner_fingerprint(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
) -> Result<Option<String>, BuildError> {
    let Some(snapshot) = read_build_runner_manifest(plan)? else {
        return Ok(None);
    };
    if snapshot.runner_source_hash != content_hash(&runner.source) {
        return Ok(None);
    }
    if !build_manifest_input_set_matches(plan, runner, &snapshot)? {
        return Ok(None);
    }
    if !std_manifest_input_set_matches(&snapshot)? {
        return Ok(None);
    }
    for input in &snapshot.inputs {
        let root = input.root.root_dir(plan);
        let Some(path) = build_runner_manifest_input_path(&root, &input.relative_path) else {
            return Ok(None);
        };
        let Ok(current) = current_build_runner_input(&input.root, &root, &path) else {
            return Ok(None);
        };
        if current.content_len != input.content_len || current.content_hash != input.content_hash {
            return Ok(None);
        }
    }
    let fingerprint = build_runner_fingerprint_from_inputs(&runner.source, &snapshot.inputs);
    if fingerprint == snapshot.fingerprint {
        Ok(Some(snapshot.fingerprint))
    } else {
        Ok(None)
    }
}

fn build_manifest_input_set_matches(
    plan: &BuildPlan,
    runner: &BuildRunnerSource,
    snapshot: &BuildRunnerFingerprintSnapshot,
) -> Result<bool, BuildError> {
    let current = build_runner_relative_input_paths(
        &plan.package_root,
        loaded_build_runner_module_files(plan, runner)?,
    )?;
    let stored = stored_manifest_relative_input_paths(snapshot, BuildRunnerFingerprintRoot::Build);
    Ok(current == stored)
}

fn std_manifest_input_set_matches(
    snapshot: &BuildRunnerFingerprintSnapshot,
) -> Result<bool, BuildError> {
    let root = workspace_std_root();
    let mut current = Vec::new();
    collect_nia_files(&root, &mut current)?;
    let current = build_runner_relative_input_paths(&root, current)?;
    let stored = stored_manifest_relative_input_paths(snapshot, BuildRunnerFingerprintRoot::Std);

    Ok(current == stored)
}

fn build_runner_relative_input_paths(
    root: &Path,
    files: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, BuildError> {
    let mut relative = files
        .into_iter()
        .map(|path| safe_relative_build_runner_input_path(root, &path))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| BuildError::BuildRunnerFingerprint {
            path: root.to_path_buf(),
            operation: "relativize",
            error: io::Error::new(
                io::ErrorKind::InvalidInput,
                "input path is outside fingerprint root",
            ),
        })?;
    relative.sort();
    relative.dedup();
    Ok(relative)
}

fn stored_manifest_relative_input_paths(
    snapshot: &BuildRunnerFingerprintSnapshot,
    root: BuildRunnerFingerprintRoot,
) -> Vec<PathBuf> {
    let mut stored = snapshot
        .inputs
        .iter()
        .filter(|input| input.root == root)
        .map(|input| input.relative_path.clone())
        .collect::<Vec<_>>();
    stored.sort();
    stored.dedup();
    stored
}

fn current_build_runner_input(
    root_kind: &BuildRunnerFingerprintRoot,
    root: &Path,
    file: &Path,
) -> Result<BuildRunnerFingerprintInput, BuildError> {
    build_runner_fingerprint_input(*root_kind, root, file)
}

fn build_runner_manifest_input_path(root: &Path, relative: &Path) -> Option<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(root.join(relative))
}

fn build_runner_manifest_path(plan: &BuildPlan) -> PathBuf {
    plan.runner_dir.join("fingerprint.manifest")
}

fn read_build_runner_manifest(
    plan: &BuildPlan,
) -> Result<Option<BuildRunnerFingerprintSnapshot>, BuildError> {
    let path = build_runner_manifest_path(plan);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BuildError::BuildRunnerFingerprint {
                path,
                operation: "read",
                error,
            });
        }
    };
    Ok(parse_build_runner_manifest(&text))
}

fn save_build_runner_manifest(
    plan: &BuildPlan,
    snapshot: &BuildRunnerFingerprintSnapshot,
) -> Result<(), BuildError> {
    let path = build_runner_manifest_path(plan);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let stage_id = SHARED_RUNNER_STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged = path.with_extension(format!("manifest.tmp.{}.{}", std::process::id(), stage_id));
    fs::write(&staged, format_build_runner_manifest(snapshot)).map_err(|error| {
        BuildError::BuildRunnerFingerprint {
            path: staged.clone(),
            operation: "write",
            error,
        }
    })?;
    match fs::rename(&staged, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(BuildError::BuildRunnerFingerprint {
                path,
                operation: "rename",
                error,
            })
        }
    }
}

fn local_build_runner_executable(plan: &BuildPlan, fingerprint: &str) -> PathBuf {
    plan.runner_dir.join(fingerprint).join("nia-build-runner")
}

fn shared_build_runner_executable(fingerprint: &str) -> PathBuf {
    let mut path = env::temp_dir();
    path.push("nia_build_runner_cache");
    path.push(build_runner_cache_namespace());
    path.push(fingerprint);
    path.push("nia-build-runner");
    path
}

fn build_runner_cache_namespace() -> String {
    let mut hash = StableFingerprint::new();
    hash.path(&workspace_std_root());
    hash.string(env!("CARGO_PKG_VERSION"));
    hash.finish()
}

fn install_shared_build_runner(
    local_runner: &Path,
    shared_runner: &Path,
) -> Result<(), BuildError> {
    if let Some(parent) = local_runner.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let stage_id = SHARED_RUNNER_STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged_runner =
        local_runner.with_extension(format!("tmp.{}.{}", std::process::id(), stage_id));
    fs::copy(shared_runner, &staged_runner).map_err(|error| {
        BuildError::BuildRunnerFingerprint {
            path: staged_runner.clone(),
            operation: "copy",
            error,
        }
    })?;
    make_executable_if_needed(&staged_runner, shared_runner)?;
    publish_runner(&staged_runner, local_runner)
}

fn save_shared_build_runner(local_runner: &Path, fingerprint: &str) -> Result<(), BuildError> {
    let shared_runner = shared_build_runner_executable(fingerprint);
    if let Some(parent) = shared_runner.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::BuildRunnerFingerprint {
            path: parent.to_path_buf(),
            operation: "create",
            error,
        })?;
    }
    let stage_id = SHARED_RUNNER_STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged_runner =
        shared_runner.with_extension(format!("tmp.{}.{}", std::process::id(), stage_id));
    fs::copy(local_runner, &staged_runner).map_err(|error| BuildError::BuildRunnerFingerprint {
        path: staged_runner.clone(),
        operation: "copy",
        error,
    })?;
    make_executable_if_needed(&staged_runner, local_runner)?;
    match fs::rename(&staged_runner, &shared_runner) {
        Ok(()) => {}
        Err(error) if shared_runner.is_file() => {
            let _ = fs::remove_file(&staged_runner);
            let _ = error;
        }
        Err(error) => {
            let _ = fs::remove_file(&staged_runner);
            return Err(BuildError::BuildRunnerFingerprint {
                path: shared_runner,
                operation: "rename",
                error,
            });
        }
    }
    Ok(())
}

fn publish_runner(staged_runner: &Path, final_runner: &Path) -> Result<(), BuildError> {
    match fs::rename(staged_runner, final_runner) {
        Ok(()) => Ok(()),
        Err(error) if final_runner.is_file() => {
            let _ = fs::remove_file(staged_runner);
            let _ = error;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(staged_runner);
            Err(BuildError::BuildRunnerFingerprint {
                path: final_runner.to_path_buf(),
                operation: "rename",
                error,
            })
        }
    }
}

fn make_executable_if_needed(path: &Path, source: &Path) -> Result<(), BuildError> {
    #[cfg(unix)]
    {
        let permissions = fs::metadata(source)
            .map_err(|error| BuildError::BuildRunnerFingerprint {
                path: source.to_path_buf(),
                operation: "metadata",
                error,
            })?
            .permissions();
        fs::set_permissions(path, permissions).map_err(|error| {
            BuildError::BuildRunnerFingerprint {
                path: path.to_path_buf(),
                operation: "chmod",
                error,
            }
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = source;
    }
    Ok(())
}

fn collect_nia_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), BuildError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(BuildError::BuildRunnerFingerprint {
                path: root.to_path_buf(),
                operation: "read",
                error,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| BuildError::BuildRunnerFingerprint {
            path: root.to_path_buf(),
            operation: "read",
            error,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| BuildError::BuildRunnerFingerprint {
                path: path.clone(),
                operation: "read",
                error,
            })?;
        if file_type.is_dir() {
            collect_nia_files(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "nia") {
            out.push(path);
        }
    }
    Ok(())
}

fn format_build_runner_manifest(snapshot: &BuildRunnerFingerprintSnapshot) -> String {
    let mut text = String::new();
    text.push_str(BUILD_RUNNER_MANIFEST_VERSION);
    text.push('\n');
    text.push_str("fingerprint\t");
    text.push_str(&snapshot.fingerprint);
    text.push('\n');
    text.push_str("runner\t");
    text.push_str(&snapshot.runner_source_hash);
    text.push('\n');
    for input in &snapshot.inputs {
        text.push_str("input\t");
        text.push_str(input.root.as_manifest_str());
        text.push('\t');
        text.push_str(&input.content_len.to_string());
        text.push('\t');
        text.push_str(&input.content_hash);
        text.push('\t');
        text.push_str(&input.relative_path.to_string_lossy());
        text.push('\n');
    }
    text
}

fn parse_build_runner_manifest(text: &str) -> Option<BuildRunnerFingerprintSnapshot> {
    let mut lines = text.lines();
    (lines.next()? == BUILD_RUNNER_MANIFEST_VERSION).then_some(())?;
    let fingerprint = lines.next()?.strip_prefix("fingerprint\t")?.to_string();
    let runner_source_hash = lines.next()?.strip_prefix("runner\t")?.to_string();
    if fingerprint.is_empty() || runner_source_hash.is_empty() {
        return None;
    }
    let mut inputs = Vec::new();
    for line in lines {
        let mut fields = line.splitn(5, '\t');
        (fields.next()? == "input").then_some(())?;
        let root = BuildRunnerFingerprintRoot::from_manifest_str(fields.next()?)?;
        let content_len = fields.next()?.parse().ok()?;
        let content_hash = fields.next()?.to_string();
        let relative_path = PathBuf::from(fields.next()?);
        if content_hash.is_empty()
            || build_runner_manifest_input_path(Path::new(""), &relative_path).is_none()
        {
            return None;
        }
        inputs.push(BuildRunnerFingerprintInput {
            root,
            relative_path,
            content_len,
            content_hash,
        });
    }
    Some(BuildRunnerFingerprintSnapshot {
        fingerprint,
        runner_source_hash,
        inputs,
    })
}

fn workspace_std_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")))
        .join("lib/std")
}

struct StableFingerprint {
    state: u64,
}

impl StableFingerprint {
    fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }

    fn string(&mut self, text: &str) {
        self.bytes(&(text.len() as u64).to_le_bytes());
        self.bytes(text.as_bytes());
    }

    fn path(&mut self, path: &Path) {
        self.string(&path.to_string_lossy());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}", self.state)
    }
}

fn content_hash(text: &str) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(text);
    hash.finish()
}

fn run_build_runner(plan: &BuildPlan, runner_executable: &Path) -> Result<(), BuildError> {
    let mut command = Command::new(runner_executable);
    command.current_dir(&plan.package_root);
    command.arg(&plan.package_root);
    command.arg(&plan.build_dir);
    command.arg(&plan.cache_dir);
    command.arg(&plan.toolchain_executable);
    if let Some(step) = plan.step.as_runner_arg() {
        command.arg(step);
    }
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(BuildError::RunnerFailed {
            path: runner_executable.to_path_buf(),
            status,
        }),
        Err(error) => Err(BuildError::RunRunner {
            path: runner_executable.to_path_buf(),
            error,
        }),
    }
}

struct BuildLock {
    path: PathBuf,
    token: String,
}

impl BuildLock {
    fn acquire(plan: &BuildPlan) -> Result<Self, BuildError> {
        const STALE_AFTER: Duration = Duration::from_secs(15 * 60);

        fs::create_dir_all(&plan.build_dir).map_err(|error| BuildError::CreateBuildDirectory {
            path: plan.build_dir.clone(),
            error,
        })?;
        let path = plan.build_dir.join(".lock");
        let start = Instant::now();
        let mut sleep = Duration::from_millis(10);
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    let token = write_lock_owner(&path, file)?;
                    return Ok(Self { path, token });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    reclaim_stale_build_lock(&path, STALE_AFTER);
                }
                Err(error) => {
                    return Err(BuildError::AcquireBuildLock {
                        path: path.clone(),
                        error,
                    });
                }
            }
            if start.elapsed() >= STALE_AFTER {
                reclaim_stale_build_lock(&path, Duration::ZERO);
            }
            thread::sleep(sleep);
            sleep = (sleep * 2).min(Duration::from_millis(250));
        }
    }
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|current| current.trim_end() == self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct SharedRunnerCompileLock {
    path: PathBuf,
    token: String,
}

impl SharedRunnerCompileLock {
    fn acquire(fingerprint: &str) -> Result<Self, BuildError> {
        const STALE_AFTER: Duration = Duration::from_secs(15 * 60);

        let shared_runner = shared_build_runner_executable(fingerprint);
        let lock_path = shared_runner.with_extension("compile.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|error| BuildError::BuildRunnerFingerprint {
                path: parent.to_path_buf(),
                operation: "create",
                error,
            })?;
        }
        let start = Instant::now();
        let mut sleep = Duration::from_millis(10);
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    let token = write_lock_owner(&lock_path, file)?;
                    return Ok(Self {
                        path: lock_path,
                        token,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    reclaim_stale_build_lock(&lock_path, STALE_AFTER);
                }
                Err(error) => {
                    return Err(BuildError::BuildRunnerFingerprint {
                        path: lock_path.clone(),
                        operation: "lock",
                        error,
                    });
                }
            }
            if start.elapsed() >= STALE_AFTER {
                reclaim_stale_build_lock(&lock_path, Duration::ZERO);
            }
            thread::sleep(sleep);
            sleep = (sleep * 2).min(Duration::from_millis(250));
        }
    }
}

impl Drop for SharedRunnerCompileLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|current| current.trim_end() == self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn write_lock_owner(path: &Path, mut file: fs::File) -> Result<String, BuildError> {
    use std::io::Write as _;

    let pid = std::process::id();
    let start_time = process_start_time(pid).unwrap_or(0);
    let token = format!("{}:{}", pid, start_time);
    writeln!(file, "{token}").map_err(|error| BuildError::AcquireBuildLock {
        path: path.to_path_buf(),
        error,
    })?;
    Ok(token)
}

fn reclaim_stale_build_lock(path: &Path, stale_after: Duration) {
    if build_lock_owner_is_alive(path) {
        return;
    };
    if read_lock_owner(path).is_none() && !build_lock_is_stale_by_age(path, stale_after) {
        return;
    }
    let _ = fs::remove_file(path);
}

fn build_lock_is_stale_by_age(path: &Path, stale_after: Duration) -> bool {
    if stale_after == Duration::ZERO {
        return true;
    }
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= stale_after)
}

#[cfg(unix)]
fn build_lock_owner_is_alive(path: &Path) -> bool {
    let Some((pid, expected_start_time)) = read_lock_owner(path) else {
        return false;
    };
    process_is_alive(pid, expected_start_time)
}

#[cfg(not(unix))]
fn build_lock_owner_is_alive(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(15 * 60))
}

fn read_lock_owner(path: &Path) -> Option<(u32, u64)> {
    let owner = fs::read_to_string(path).ok()?;
    let token = owner.split_whitespace().next()?;
    let (pid, start_time) = token.split_once(':')?;
    Some((pid.parse().ok()?, start_time.parse().ok()?))
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32, expected_start_time: u64) -> bool {
    let Some(start_time) = process_start_time(pid) else {
        return false;
    };
    expected_start_time == start_time
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_is_alive(_pid: u32, _expected_start_time: u64) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

fn find_package_root(start: &Path) -> Result<PathBuf, BuildError> {
    let mut cursor = start.to_path_buf();
    loop {
        if cursor.join("build.nia").is_file() {
            return Ok(cursor);
        }
        if !cursor.pop() {
            return Err(BuildError::MissingBuildScript {
                start: start.to_path_buf(),
            });
        }
    }
}

fn time_stage<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    if !timings.enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!("timing {name}: {:.3}s", start.elapsed().as_secs_f64());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_package_root_from_child_directory() {
        let root = temp_root("resolves_package_root_from_child_directory");
        let child = root.join("src").join("nested");
        std::fs::create_dir_all(&child).expect("create child");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new().with_root(&child)).expect("build plan");

        assert_eq!(plan.package_root, root);
        assert_eq!(plan.build_script, plan.package_root.join("build.nia"));
        assert!(plan.toolchain_executable.is_file());
        assert_eq!(plan.build_dir, plan.package_root.join(".nia-build"));
        assert_eq!(plan.cache_dir, plan.package_root.join(".nia-cache"));
        assert_eq!(plan.runner_dir, plan.package_root.join(".nia-build/runner"));
        assert_eq!(
            plan.runner_executable,
            plan.package_root.join(".nia-build/runner/nia-build-runner")
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
    }

    #[test]
    fn generated_runner_invokes_build_script_as_normal_nia_module() {
        let root = temp_root("generated_runner_invokes_build_script_as_normal_nia_module");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");

        let runner = build_runner_source(&plan).expect("build runner source");

        assert_eq!(
            runner.path,
            plan.runner_dir.join("root.nia").to_string_lossy()
        );
        assert!(runner.source.contains("using std::build;"));
        assert!(runner.source.contains("using std::fs;"));
        assert!(runner.source.contains("using std::mem;"));
        assert!(runner.source.contains("using build_script;"));
        assert!(runner.source.contains("fn path_arg("));
        assert!(runner.source.contains("fs::PathView::from_utf8_into("));
        assert!(runner.source.contains("let mut api = build::Build::init("));
        assert!(
            runner
                .source
                .contains("path_arg(init, &mut allocator, 1usize")
        );
        assert!(
            runner
                .source
                .contains("path_arg(init, &mut allocator, 2usize")
        );
        assert!(
            runner
                .source
                .contains("path_arg(init, &mut allocator, 3usize")
        );
        assert!(
            runner
                .source
                .contains("path_arg(init, &mut allocator, 4usize")
        );
        assert!(runner.source.contains("5usize,"));
        assert!(
            runner
                .source
                .contains("build_script::build(&mut api).exit().?;")
        );
        assert!(!runner.source.contains("comptime"));
    }

    #[test]
    fn preserves_named_step() {
        let root = temp_root("preserves_named_step");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new().with_root(&root).with_step("install"))
            .expect("build plan");

        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
        assert_eq!(plan.step.as_runner_arg(), Some("install"));
    }

    #[test]
    fn reports_missing_build_script_from_start_directory() {
        let root = temp_root("reports_missing_build_script_from_start_directory");

        let error = resolve_build_plan(BuildRequest::new().with_root(&root))
            .expect_err("missing build script");

        assert!(matches!(error, BuildError::MissingBuildScript { start } if start == root));
    }

    #[test]
    fn prepares_build_and_cache_directories() {
        let root = temp_root("prepares_build_and_cache_directories");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");

        prepare_build_directories(&plan).expect("prepare build directories");

        assert!(plan.build_dir.is_dir());
        assert!(plan.cache_dir.is_dir());
    }

    #[test]
    fn build_lock_serializes_same_package_root() {
        let root = temp_root("build_lock_serializes_same_package_root");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let first = BuildLock::acquire(&plan).expect("first build lock");
        let second_plan = plan.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            ready_tx.send(()).expect("send ready");
            let second = BuildLock::acquire(&second_plan).expect("second build lock");
            release_tx.send(()).expect("send acquired");
            drop(second);
        });

        ready_rx.recv().expect("second thread ready");
        assert!(
            release_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("second lock acquired after first release");
        handle.join().expect("second lock thread");
    }

    #[test]
    fn parses_build_lock_owner_token() {
        let root = temp_root("parses_build_lock_owner_token");
        let lock = root.join(".lock");
        std::fs::write(&lock, "123:456\n").expect("write lock");

        assert_eq!(read_lock_owner(&lock), Some((123, 456)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn build_lock_owner_rejects_reused_pid_with_different_start_time() {
        let root = temp_root("build_lock_owner_rejects_reused_pid_with_different_start_time");
        let lock = root.join(".lock");
        let pid = std::process::id();
        let current_start_time = process_start_time(pid).expect("current process start time");
        std::fs::write(&lock, format!("{pid}:{}\n", current_start_time + 1)).expect("write lock");

        assert!(!build_lock_owner_is_alive(&lock));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reclaim_stale_build_lock_removes_dead_owner_without_age_delay() {
        let root = temp_root("reclaim_stale_build_lock_removes_dead_owner_without_age_delay");
        let lock = root.join(".lock");
        let pid = std::process::id();
        let current_start_time = process_start_time(pid).expect("current process start time");
        std::fs::write(&lock, format!("{pid}:{}\n", current_start_time + 1)).expect("write lock");

        reclaim_stale_build_lock(&lock, Duration::from_secs(15 * 60));

        assert!(!lock.exists());
    }

    #[test]
    fn build_runner_fingerprint_ignores_non_build_graph_package_sources() {
        let root = temp_root("build_runner_fingerprint_ignores_non_build_graph_package_sources");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(root.join("build.nia"), "using std::build;\n").expect("write build script");
        std::fs::write(root.join("src/main.nia"), "fn main() void {}\n").expect("write app");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let before = build_runner_fingerprint(&plan, &runner).expect("fingerprint before");

        std::fs::write(root.join("src/main.nia"), "fn main() i32 { 1 }\n").expect("edit app");
        let after = build_runner_fingerprint(&plan, &runner).expect("fingerprint after");

        assert_eq!(before.fingerprint, after.fingerprint);
    }

    #[test]
    fn build_runner_fingerprint_tracks_build_script_module_graph_sources() {
        let root = temp_root("build_runner_fingerprint_tracks_build_script_module_graph_sources");
        std::fs::create_dir_all(root.join("build")).expect("create build module dir");
        std::fs::write(
            root.join("build.nia"),
            "module helper;\nusing std::build;\n",
        )
        .expect("write build script");
        std::fs::write(root.join("build/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let before = build_runner_fingerprint(&plan, &runner).expect("fingerprint before");

        std::fs::write(root.join("build/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");
        let after = build_runner_fingerprint(&plan, &runner).expect("fingerprint after");

        assert_ne!(before.fingerprint, after.fingerprint);
    }

    #[test]
    fn build_runner_fingerprint_tracks_nested_declared_build_modules() {
        let root = temp_root("build_runner_fingerprint_tracks_nested_declared_build_modules");
        std::fs::create_dir_all(root.join("build").join("helper"))
            .expect("create build module dir");
        std::fs::write(root.join("build.nia"), "module helper;\n").expect("write build script");
        std::fs::write(root.join("build/helper.nia"), "module nested;\n").expect("write helper");
        std::fs::write(
            root.join("build/helper/nested.nia"),
            "pub fn value() i32 { 1 }\n",
        )
        .expect("write nested helper");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let before = build_runner_fingerprint(&plan, &runner).expect("fingerprint before");

        std::fs::write(
            root.join("build/helper/nested.nia"),
            "pub fn value() i32 { 2 }\n",
        )
        .expect("edit nested helper");
        let after = build_runner_fingerprint(&plan, &runner).expect("fingerprint after");

        assert_ne!(before.fingerprint, after.fingerprint);
    }

    #[test]
    fn build_runner_fingerprint_is_independent_of_package_root_path() {
        let first = temp_root("build_runner_fingerprint_is_independent_of_package_root_path_first");
        let second =
            temp_root("build_runner_fingerprint_is_independent_of_package_root_path_second");
        std::fs::create_dir_all(first.join("src")).expect("create first src");
        std::fs::create_dir_all(second.join("src")).expect("create second src");
        for root in [&first, &second] {
            std::fs::write(root.join("build.nia"), "using std::build;\n")
                .expect("write build script");
            std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 1 }\n")
                .expect("write helper");
        }
        let first_plan =
            resolve_build_plan(BuildRequest::new().with_root(&first)).expect("first build plan");
        let second_plan =
            resolve_build_plan(BuildRequest::new().with_root(&second)).expect("second build plan");
        let first_runner = build_runner_source(&first_plan).expect("first runner source");
        let second_runner = build_runner_source(&second_plan).expect("second runner source");

        assert_eq!(
            build_runner_fingerprint(&first_plan, &first_runner)
                .expect("first fingerprint")
                .fingerprint,
            build_runner_fingerprint(&second_plan, &second_runner)
                .expect("second fingerprint")
                .fingerprint,
        );
    }

    #[test]
    fn build_runner_manifest_restores_unchanged_fingerprint() {
        let root = temp_root("build_runner_manifest_restores_unchanged_fingerprint");
        std::fs::write(root.join("build.nia"), "using std::build;\n").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let snapshot = build_runner_fingerprint(&plan, &runner).expect("fingerprint");
        save_build_runner_manifest(&plan, &snapshot).expect("save manifest");

        assert_eq!(
            restore_build_runner_fingerprint(&plan, &runner).expect("restore manifest"),
            Some(snapshot.fingerprint)
        );
    }

    #[test]
    fn build_runner_manifest_rejects_changed_build_graph_input() {
        let root = temp_root("build_runner_manifest_rejects_changed_build_graph_input");
        std::fs::create_dir_all(root.join("build")).expect("create build module dir");
        std::fs::write(
            root.join("build.nia"),
            "module helper;\nusing std::build;\n",
        )
        .expect("write build script");
        std::fs::write(root.join("build/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let snapshot = build_runner_fingerprint(&plan, &runner).expect("fingerprint");
        save_build_runner_manifest(&plan, &snapshot).expect("save manifest");

        std::fs::write(root.join("build/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");

        assert_eq!(
            restore_build_runner_fingerprint(&plan, &runner).expect("restore manifest"),
            None
        );
    }

    #[test]
    fn build_runner_manifest_rejects_changed_runner_source() {
        let root = temp_root("build_runner_manifest_rejects_changed_runner_source");
        std::fs::write(root.join("build.nia"), "using std::build;\n").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");
        let snapshot = build_runner_fingerprint(&plan, &runner).expect("fingerprint");
        save_build_runner_manifest(&plan, &snapshot).expect("save manifest");
        let mut changed_runner = runner.clone();
        changed_runner.source.push_str("\n");

        assert_eq!(
            restore_build_runner_fingerprint(&plan, &changed_runner).expect("restore manifest"),
            None
        );
    }

    #[test]
    fn build_runner_cache_is_keyed_by_fingerprint_path() {
        let root = temp_root("build_runner_cache_is_keyed_by_fingerprint_path");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new().with_root(&root)).expect("build plan");
        let first = local_build_runner_executable(&plan, "abc");
        let second = local_build_runner_executable(&plan, "def");

        assert_ne!(first, second);
        assert!(!build_runner_cache_valid(&first));

        std::fs::create_dir_all(first.parent().expect("runner parent")).expect("create runner dir");
        std::fs::write(&first, "").expect("write runner executable");

        assert!(build_runner_cache_valid(&first));
        assert!(!build_runner_cache_valid(&second));
    }

    #[test]
    fn shared_runner_cache_namespace_is_toolchain_family_stable() {
        let first = build_runner_cache_namespace();
        let second = build_runner_cache_namespace();

        assert_eq!(first, second);
    }

    #[test]
    fn generated_runner_source_does_not_embed_package_paths() {
        let root = temp_root("generated_runner_source_does_not_embed_package_paths");
        let package_root = root.join("quote\"slash\\tab\t");
        std::fs::create_dir_all(&package_root).expect("create package root");
        std::fs::write(package_root.join("build.nia"), "").expect("write build script");

        let plan =
            resolve_build_plan(BuildRequest::new().with_root(&package_root)).expect("build plan");
        let runner = build_runner_source(&plan).expect("build runner source");

        assert!(
            !runner
                .source
                .contains(&package_root.to_string_lossy().to_string())
        );
        assert!(!runner.source.contains("quote\\\"slash"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("nia-build-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("remove old temp root");
        }
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
