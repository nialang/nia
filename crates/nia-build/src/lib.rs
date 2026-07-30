// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env,
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, LinkExecutableRequest, TimingMode,
};
use nia_imports::ModuleMap;
use nia_source::SourcePath;
use nia_timing::{TimingFormat, TimingOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    pub root: Option<PathBuf>,
    pub step: Option<String>,
    pub timings: TimingMode,
    pub timing_format: TimingFormat,
}

impl BuildRequest {
    pub fn new(toolchain: Arc<nia_toolchain::ToolchainLayout>) -> Self {
        Self {
            toolchain,
            root: None,
            step: None,
            timings: TimingMode::Off,
            timing_format: TimingFormat::Text,
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

    pub fn with_timing_format(mut self, timing_format: TimingFormat) -> Self {
        self.timing_format = timing_format;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    pub package_root: PathBuf,
    pub build_script: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runner_dir: PathBuf,
    pub runner_executable: PathBuf,
    pub step: BuildStepSelection,
    pub timings: TimingMode,
    pub timing_format: TimingFormat,
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

#[derive(Debug)]
pub enum BuildError {
    CurrentDirectory {
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
        error: Box<DriverError>,
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
    nia_timing::collect_to_stderr(
        TimingOptions::new(request.timings).with_format(request.timing_format),
        || {
            let timings = request.timings;
            let plan = time_summary_stage(timings, "build_resolve_plan", || {
                resolve_build_plan(request)
            })?;
            time_summary_stage(timings, "build_prepare_directories", || {
                prepare_build_directories(&plan)
            })?;
            nia_timing::emit_counter("build.runner_compilations", 1);
            let runner_executable = time_summary_stage(timings, "build_compile_runner", || {
                compile_build_runner(&plan)
            });
            if runner_executable.is_err() {
                nia_timing::emit_counter("build.runner_compile_failures", 1);
            }
            let runner_executable = runner_executable?;
            let _lock =
                time_summary_stage(timings, "build_acquire_lock", || BuildLock::acquire(&plan))?;
            nia_timing::emit_counter("build.runner_executions", 1);
            let result = time_summary_stage(timings, "build_run_runner", || {
                run_build_runner(&plan, &runner_executable)
            });
            if result.is_err() {
                nia_timing::emit_counter("build.runner_failures", 1);
            }
            result
        },
    )
}

pub fn resolve_build_plan(request: BuildRequest) -> Result<BuildPlan, BuildError> {
    let start = match request.root {
        Some(root) => root,
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let package_root = find_package_root(&start)?;
    let build_script = package_root.join("build.nia");
    let build_dir = package_root.join(".nia-build");
    let runner_dir = build_dir.join("runner");
    Ok(BuildPlan {
        toolchain: request.toolchain,
        cache_dir: package_root.join(".nia-cache"),
        runner_executable: runner_dir.join("nia-build-runner"),
        runner_dir,
        build_dir,
        package_root,
        build_script,
        step: request
            .step
            .map(BuildStepSelection::Named)
            .unwrap_or(BuildStepSelection::Default),
        timings: request.timings,
        timing_format: request.timing_format,
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
using std::fmt;
using std::fs;
using std::mem;
using std::process;
using buildScript;

struct TargetText {
    arch: collections::ArrayList[char],
    vendor: collections::ArrayList[char],
    os: collections::ArrayList[char],
    env: collections::ArrayList[char],
    abi: collections::ArrayList[char],
    endian: collections::ArrayList[char],
}

fn rememberTargetCleanupError(
    firstError: &mut ?build::Error,
    result: build::Error!void,
) void {
    switch result {
        !ok => {
            _ = ok;
        },
        error! => {
            if firstError.* is null {
                firstError.* = ?error;
            }
        },
    }
}

extend TargetText {
    fn init() TargetText {
        {
            arch: collections::ArrayList[char]::init(),
            vendor: collections::ArrayList[char]::init(),
            os: collections::ArrayList[char]::init(),
            env: collections::ArrayList[char]::init(),
            abi: collections::ArrayList[char]::init(),
            endian: collections::ArrayList[char]::init(),
        }
    }

    fn deinit(&mut self, allocator: &mut mem::Allocator) build::Error!void {
        let mut firstError: ?build::Error = null;
        rememberTargetCleanupError(&mut firstError, self.endian.deinit(allocator).asBuildError());
        rememberTargetCleanupError(&mut firstError, self.abi.deinit(allocator).asBuildError());
        rememberTargetCleanupError(&mut firstError, self.env.deinit(allocator).asBuildError());
        rememberTargetCleanupError(&mut firstError, self.os.deinit(allocator).asBuildError());
        rememberTargetCleanupError(&mut firstError, self.vendor.deinit(allocator).asBuildError());
        rememberTargetCleanupError(&mut firstError, self.arch.deinit(allocator).asBuildError());
        if firstError is ?error {
            return error!;
        }
        !{}
    }
}

fn pathArg(
    init: process::Init,
    allocator: &mut mem::Allocator,
    index: usize,
    storage: &mut collections::ArrayList[char],
) build::Error!fs::PathView {
    let arg = switch init.args().get(index) {
        ?value => {
            value
        },
        null => {
            return build::Error::Internal!;
        },
    };
    fs::PathView::from_utf8_into(allocator, arg.bytes(), storage).asBuildError()
}

fn targetArg(
    init: process::Init,
    allocator: &mut mem::Allocator,
    startIndex: usize,
    storage: &mut TargetText,
) build::Error!build::TargetView {
    let arch = pathArg(init, allocator, startIndex, &mut storage.arch).?.string();
    let vendor = pathArg(init, allocator, startIndex + 1usize, &mut storage.vendor).?.string();
    let os = pathArg(init, allocator, startIndex + 2usize, &mut storage.os).?.string();
    let env = pathArg(init, allocator, startIndex + 3usize, &mut storage.env).?.string();
    let abi = pathArg(init, allocator, startIndex + 4usize, &mut storage.abi).?.string();
    let endian = pathArg(init, allocator, startIndex + 5usize, &mut storage.endian).?.string();
    let pointerWidthArg = switch init.args().get(startIndex + 6usize) {
        ?value => {
            value
        },
        null => {
            return build::Error::Internal!;
        },
    };
    let pointerWidth = switch fmt::parse[u32](pointerWidthArg) {
        !value => {
            value
        },
        error! => {
            _ = error;
            return build::Error::InvalidTarget!;
        },
    };
    !build::TargetView::init(arch, vendor, os, env, abi, endian, pointerWidth)
}

fn actionReportEnabled(init: process::Init) bool {
    switch init.args().get(6usize) {
        ?arg => {
            let bytes = arg.bytes();
            bytes.len() == 4usize
                and bytes[0usize] == b'j'
                and bytes[1usize] == b's'
                and bytes[2usize] == b'o'
                and bytes[3usize] == b'n'
        },
        null => {
            false
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut pageAllocator);
    defer allocator.deinit().ok().exit().?;

    let mut packageRootText = collections::ArrayList[char]::init();
    defer packageRootText.deinit(&mut allocator).exit().?;
    let mut buildDirText = collections::ArrayList[char]::init();
    defer buildDirText.deinit(&mut allocator).exit().?;
    let mut cacheDirText = collections::ArrayList[char]::init();
    defer cacheDirText.deinit(&mut allocator).exit().?;
    let mut toolchainText = collections::ArrayList[char]::init();
    defer toolchainText.deinit(&mut allocator).exit().?;
    let mut resourceRootText = collections::ArrayList[char]::init();
    defer resourceRootText.deinit(&mut allocator).exit().?;

    let packageRoot = pathArg(init, &mut allocator, 1usize, &mut packageRootText).exit().?;
    let buildDir = pathArg(init, &mut allocator, 2usize, &mut buildDirText).exit().?;
    let cacheDir = pathArg(init, &mut allocator, 3usize, &mut cacheDirText).exit().?;
    let toolchainExecutable = pathArg(init, &mut allocator, 4usize, &mut toolchainText).exit().?;
    let toolchainResourceRoot = pathArg(init, &mut allocator, 5usize, &mut resourceRootText).exit().?;
    let mut hostTargetText = TargetText::init();
    defer hostTargetText.deinit(&mut allocator).exit().?;
    let hostTarget = targetArg(init, &mut allocator, 7usize, &mut hostTargetText).exit().?;
    let mut artifactTargetText = TargetText::init();
    defer artifactTargetText.deinit(&mut allocator).exit().?;
    let artifactTarget = targetArg(init, &mut allocator, 14usize, &mut artifactTargetText).exit().?;

    let mut api = build::Build::init(
        init,
        &mut allocator,
"#,
    );
    source.push_str(
        r#"        packageRoot,
        buildDir,
        cacheDir,
        toolchainExecutable,
        toolchainResourceRoot,
        hostTarget,
        artifactTarget,
        actionReportEnabled(init),
        21usize,
"#,
    );
    source.push_str(
        r#"    ).exit().?;
    defer api.deinit().exit().?;

    switch buildScript::build(&mut api) {
        !ok => {
            _ = ok;
        },
        error! => {
            switch api.reportActions(false) {
                !reported => {
                    _ = reported;
                },
                reportError! => {
                    _ = reportError;
                },
            }
            return error.asExitCode()!;
        },
    }
    switch api.runRequestedStep() {
        !ok => {
            _ = ok;
        },
        error! => {
            switch api.reportActions(false) {
                !reported => {
                    _ = reported;
                },
                reportError! => {
                    _ = reportError;
                },
            }
            return error.asExitCode()!;
        },
    }
    switch api.reportActions(true) {
        !reported => {
            _ = reported;
        },
        reportError! => {
            _ = reportError;
        },
    }
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
    if let Some(parent) = plan.runner_executable.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let driver = Driver::with_config(build_runner_driver_config(plan));
    driver.set_source(runner.path.clone(), runner.source.clone());
    let output = driver.link_executable(LinkExecutableRequest::new(
        CheckRequest::new(runner.path.clone())
            .with_module_map(build_runner_module_map(plan))
            .with_timings(plan.timings),
        &plan.runner_executable,
    ));
    output.result.map_err(|error| BuildError::CompileRunner {
        path: runner.path,
        source: runner.source,
        error: Box::new(error),
    })?;
    Ok(plan.runner_executable.clone())
}

fn build_runner_driver_config(plan: &BuildPlan) -> DriverConfig {
    DriverConfig {
        artifact_cache_dir: Some(plan.cache_dir.clone()),
        ..DriverConfig::new(Arc::clone(&plan.toolchain))
    }
    .with_artifact_target(plan.toolchain.host_target().clone())
}

fn build_runner_module_map(plan: &BuildPlan) -> ModuleMap {
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "buildScript",
        SourcePath::new(plan.build_script.to_string_lossy().into_owned()),
    );
    module_map
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

fn run_build_runner(plan: &BuildPlan, runner_executable: &Path) -> Result<(), BuildError> {
    let mut command = Command::new(runner_executable);
    command.current_dir(&plan.package_root);
    command.args(build_runner_args(plan));
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

fn build_runner_args(plan: &BuildPlan) -> Vec<OsString> {
    let mut args = vec![
        plan.package_root.as_os_str().to_owned(),
        plan.build_dir.as_os_str().to_owned(),
        plan.cache_dir.as_os_str().to_owned(),
        plan.toolchain.compiler_executable().as_os_str().to_owned(),
        plan.toolchain.resource_root().as_os_str().to_owned(),
        OsString::from(
            if plan.timings == TimingMode::Detail && plan.timing_format == TimingFormat::Json {
                "json"
            } else {
                "off"
            },
        ),
    ];
    args.extend(target_runner_args(plan.toolchain.host_target()).map(OsString::from));
    args.extend(target_runner_args(plan.toolchain.artifact_target()).map(OsString::from));
    if let Some(step) = plan.step.as_runner_arg() {
        args.push(OsString::from(step));
    }
    args
}

fn target_runner_args(target: &nia_target_config::TargetConfig) -> [String; 7] {
    [
        target.arch.clone(),
        target.vendor.clone(),
        target.os.clone(),
        target.env.clone(),
        target.abi.clone(),
        target.endian.clone(),
        target.pointer_width.to_string(),
    ]
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

fn time_summary_stage<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_stage(timings, nia_timing::TimingLevel::Summary, name, f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn test_toolchain_layout() -> Arc<nia_toolchain::ToolchainLayout> {
        static LAYOUT: OnceLock<Arc<nia_toolchain::ToolchainLayout>> = OnceLock::new();
        Arc::clone(LAYOUT.get_or_init(|| {
            let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = manifest_dir
                .parent()
                .and_then(Path::parent)
                .expect("nia-build lives under crates/");
            Arc::new(
                nia_toolchain::ToolchainLayout::resolve(
                    nia_toolchain::ToolchainLayoutRequest::explicit(
                        std::env::current_exe().expect("test executable path"),
                        workspace_root.join("lib"),
                    ),
                )
                .expect("development toolchain layout"),
            )
        }))
    }

    fn test_toolchain_layout_for(
        artifact_target: nia_target_config::TargetConfig,
    ) -> Arc<nia_toolchain::ToolchainLayout> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("nia-build lives under crates/");
        Arc::new(
            nia_toolchain::ToolchainLayout::resolve(
                nia_toolchain::ToolchainLayoutRequest::explicit(
                    std::env::current_exe().expect("test executable path"),
                    workspace_root.join("lib"),
                )
                .with_artifact_target(artifact_target),
            )
            .expect("development toolchain layout"),
        )
    }

    #[test]
    fn resolves_package_root_from_child_directory() {
        let root = temp_root("resolves_package_root_from_child_directory");
        let child = root.join("src").join("nested");
        std::fs::create_dir_all(&child).expect("create child");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&child))
            .expect("build plan");

        assert_eq!(plan.package_root, root);
        assert_eq!(plan.build_script, plan.package_root.join("build.nia"));
        assert!(plan.toolchain.compiler_executable().is_file());
        assert!(plan.toolchain.resource_root().is_dir());
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
        let plan = resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&root))
            .expect("build plan");

        let runner = build_runner_source(&plan).expect("build runner source");

        assert_eq!(
            runner.path,
            plan.runner_dir.join("root.nia").to_string_lossy()
        );
        assert!(runner.source.contains("using std::build;"));
        assert!(runner.source.contains("using std::fs;"));
        assert!(runner.source.contains("using std::mem;"));
        assert!(runner.source.contains("using buildScript;"));
        assert!(runner.source.contains("fn pathArg("));
        assert!(runner.source.contains("fn targetArg("));
        assert!(runner.source.contains("fs::PathView::from_utf8_into("));
        assert!(runner.source.contains("let mut api = build::Build::init("));
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 1usize")
        );
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 2usize")
        );
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 3usize")
        );
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 4usize")
        );
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 5usize")
        );
        assert!(runner.source.contains("actionReportEnabled(init),"));
        assert!(
            runner
                .source
                .contains("targetArg(init, &mut allocator, 7usize, &mut hostTargetText)")
        );
        assert!(
            runner
                .source
                .contains("targetArg(init, &mut allocator, 14usize, &mut artifactTargetText)")
        );
        assert!(runner.source.contains("hostTarget,"));
        assert!(runner.source.contains("artifactTarget,"));
        assert!(runner.source.contains("21usize,"));
        assert!(runner.source.contains("api.reportActions(false)"));
        assert!(runner.source.contains("api.reportActions(true)"));
        assert!(
            runner
                .source
                .contains("switch buildScript::build(&mut api)")
        );
        assert!(runner.source.contains("return error.asExitCode()!;"));
        assert!(!runner.source.contains("const"));
    }

    #[test]
    fn build_runner_compiles_for_host_and_transports_both_targets() {
        let mut artifact_target = nia_target_config::TargetConfig::host();
        artifact_target.arch = "artifact-arch".to_string();
        artifact_target.vendor = "artifact-vendor".to_string();
        artifact_target.os = "artifact-os".to_string();
        artifact_target.env = "artifact-env".to_string();
        artifact_target.abi = "artifact-abi".to_string();
        artifact_target.endian = "big".to_string();
        artifact_target.pointer_width = 32;
        let toolchain = test_toolchain_layout_for(artifact_target.clone());
        let root = temp_root("build_runner_compiles_for_host_and_transports_both_targets");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(
            BuildRequest::new(Arc::clone(&toolchain))
                .with_root(&root)
                .with_step("inspect"),
        )
        .expect("build plan");

        let config = build_runner_driver_config(&plan);
        let args = build_runner_args(&plan);

        assert_eq!(config.artifact_target, *toolchain.host_target());
        assert_ne!(config.artifact_target, *toolchain.artifact_target());
        assert_eq!(args.len(), 21);
        assert_eq!(args[6], OsString::from(&toolchain.host_target().arch));
        assert_eq!(
            args[12],
            OsString::from(toolchain.host_target().pointer_width.to_string())
        );
        assert_eq!(args[13], OsString::from("artifact-arch"));
        assert_eq!(args[14], OsString::from("artifact-vendor"));
        assert_eq!(args[15], OsString::from("artifact-os"));
        assert_eq!(args[16], OsString::from("artifact-env"));
        assert_eq!(args[17], OsString::from("artifact-abi"));
        assert_eq!(args[18], OsString::from("big"));
        assert_eq!(args[19], OsString::from("32"));
        assert_eq!(args[20], OsString::from("inspect"));
    }

    #[test]
    fn preserves_named_step() {
        let root = temp_root("preserves_named_step");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_plan(
            BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_step("install"),
        )
        .expect("build plan");

        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
        assert_eq!(plan.step.as_runner_arg(), Some("install"));
    }

    #[test]
    fn reports_missing_build_script_from_start_directory() {
        let root = temp_root("reports_missing_build_script_from_start_directory");

        let error = resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&root))
            .expect_err("missing build script");

        assert!(matches!(error, BuildError::MissingBuildScript { start } if start == root));
    }

    #[test]
    fn prepares_build_and_cache_directories() {
        let root = temp_root("prepares_build_and_cache_directories");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&root))
            .expect("build plan");

        prepare_build_directories(&plan).expect("prepare build directories");

        assert!(plan.build_dir.is_dir());
        assert!(plan.cache_dir.is_dir());
    }

    #[test]
    fn build_lock_serializes_same_package_root() {
        let root = temp_root("build_lock_serializes_same_package_root");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan = resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&root))
            .expect("build plan");
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
    fn generated_runner_source_does_not_embed_package_paths() {
        let root = temp_root("generated_runner_source_does_not_embed_package_paths");
        let package_root = root.join("quote\"slash\\tab\t");
        std::fs::create_dir_all(&package_root).expect("create package root");
        std::fs::write(package_root.join("build.nia"), "").expect("write build script");

        let plan =
            resolve_build_plan(BuildRequest::new(test_toolchain_layout()).with_root(&package_root))
                .expect("build plan");
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
