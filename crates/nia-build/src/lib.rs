// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env,
    ffi::OsString,
    fmt, fs, io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, LinkExecutableRequest, TimingMode,
};
use nia_imports::ModuleMap;
use nia_source::SourcePath;
use nia_timing::{TimingFormat, TimingOptions};

mod action_cache;
mod coordinator;
mod lock;
mod output_recovery;
mod plan;
mod resources;

pub use action_cache::{
    ActionCacheInvalidation, ActionCacheMissReason, ActionCacheOutcome, ActionCacheReport,
};
pub use coordinator::*;
pub use output_recovery::OutputRecoveryError;
pub use plan::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    pub root: Option<PathBuf>,
    pub step: Option<String>,
    pub timings: TimingMode,
    pub timing_format: TimingFormat,
    pub max_parallel_actions: Option<NonZeroUsize>,
}

impl BuildRequest {
    pub fn new(toolchain: Arc<nia_toolchain::ToolchainLayout>) -> Self {
        Self {
            toolchain,
            root: None,
            step: None,
            timings: TimingMode::Off,
            timing_format: TimingFormat::Text,
            max_parallel_actions: None,
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

    pub fn with_max_parallel_actions(mut self, max_parallel_actions: NonZeroUsize) -> Self {
        self.max_parallel_actions = Some(max_parallel_actions);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInvocation {
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    pub package_root: PathBuf,
    pub build_script: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runner_dir: PathBuf,
    pub runner_executable: PathBuf,
    pub plan_draft: PathBuf,
    pub plan_path: PathBuf,
    pub step: BuildStepSelection,
    pub timings: TimingMode,
    pub timing_format: TimingFormat,
    pub max_parallel_actions: Option<NonZeroUsize>,
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
    PreparePlanDraft {
        path: PathBuf,
        error: io::Error,
    },
    ReadPlanDraft {
        path: PathBuf,
        error: PlanHandoffError,
    },
    PublishBuildPlan {
        path: PathBuf,
        error: PlanHandoffError,
    },
    ExecuteBuildPlan {
        error: Box<CoordinatorError>,
    },
    CleanupPlanDraft {
        path: PathBuf,
        error: io::Error,
    },
    CleanupRunnerExecutable {
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
            Self::PreparePlanDraft { path, error } => write!(
                f,
                "failed to prepare build-plan draft `{}`: {error}",
                path.display()
            ),
            Self::ReadPlanDraft { path, error } => write!(
                f,
                "failed to read build-plan draft `{}`: {error}",
                path.display()
            ),
            Self::PublishBuildPlan { path, error } => write!(
                f,
                "failed to publish canonical build plan `{}`: {error}",
                path.display()
            ),
            Self::ExecuteBuildPlan { error } => write!(f, "failed to execute build plan: {error}"),
            Self::CleanupPlanDraft { path, error } => write!(
                f,
                "failed to remove build-plan draft `{}`: {error}",
                path.display()
            ),
            Self::CleanupRunnerExecutable { path, error } => {
                write!(
                    f,
                    "failed to remove transient build runner `{}`: {error}",
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
            let invocation = time_summary_stage(timings, "build_resolve_invocation", || {
                resolve_build_invocation(request)
            })?;
            time_summary_stage(timings, "build_prepare_directories", || {
                prepare_build_directories(&invocation)
            })?;
            nia_timing::emit_counter("build.runner_compilations", 1);
            let runner_executable = time_summary_stage(timings, "build_compile_runner", || {
                compile_build_runner(&invocation)
            });
            if runner_executable.is_err() {
                nia_timing::emit_counter("build.runner_compile_failures", 1);
            }
            let runner_executable = runner_executable?;
            nia_timing::emit_counter("build.runner_executions", 1);
            let plan = time_summary_stage(timings, "build_run_runner", || {
                run_and_cleanup_build_runner(&invocation, &runner_executable)
            });
            if plan.is_err() {
                nia_timing::emit_counter("build.runner_failures", 1);
            }
            let plan = plan?;
            if let Some(limit) = invocation.max_parallel_actions {
                nia_timing::emit_counter("build.action_parallelism_limit", limit.get() as u64);
            }
            let result = time_summary_stage(timings, "build_execute_plan", || {
                execute_build_plan(&plan, &invocation).map_err(|error| {
                    BuildError::ExecuteBuildPlan {
                        error: Box::new(error),
                    }
                })
            });
            if let Ok(report) = &result {
                nia_timing::emit_counter("build.steps_executed", report.steps.len() as u64);
                nia_timing::emit_counter("build.actions_executed", report.actions.len() as u64);
                emit_action_cache_counters(report);
            } else {
                nia_timing::emit_counter("build.action_failures", 1);
            }
            result.map(|_| ())
        },
    )
}

fn emit_action_cache_counters(report: &ExecutionReport) {
    nia_timing::emit_counter(
        "build.action_cache_lookups",
        report.action_cache.len() as u64,
    );
    for entry in &report.action_cache {
        match &entry.outcome {
            ActionCacheOutcome::Hit => nia_timing::emit_counter("build.action_cache_hits", 1),
            ActionCacheOutcome::Miss(reason) => {
                nia_timing::emit_counter("build.action_cache_misses", 1);
                match reason {
                    ActionCacheMissReason::NotFound => {
                        nia_timing::emit_counter("build.action_cache_miss_not_found", 1);
                    }
                    ActionCacheMissReason::Uncacheable => {
                        nia_timing::emit_counter("build.action_cache_miss_uncacheable", 1);
                    }
                    ActionCacheMissReason::Invalidated(reasons) => {
                        nia_timing::emit_counter("build.action_cache_miss_invalidated", 1);
                        for reason in reasons {
                            match reason {
                                ActionCacheInvalidation::Contents => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_contents",
                                    1,
                                ),
                                ActionCacheInvalidation::Artifact => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_artifact",
                                    1,
                                ),
                                ActionCacheInvalidation::Sources => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_sources",
                                    1,
                                ),
                                ActionCacheInvalidation::Module => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_module",
                                    1,
                                ),
                                ActionCacheInvalidation::Target => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_target",
                                    1,
                                ),
                                ActionCacheInvalidation::Optimization => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_optimization",
                                    1,
                                ),
                                ActionCacheInvalidation::Runtime => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_runtime",
                                    1,
                                ),
                                ActionCacheInvalidation::Linker => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_linker",
                                    1,
                                ),
                                ActionCacheInvalidation::Output => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_output",
                                    1,
                                ),
                                ActionCacheInvalidation::Compiler => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_compiler",
                                    1,
                                ),
                                ActionCacheInvalidation::ResourceLayout => {
                                    nia_timing::emit_counter(
                                        "build.action_cache_invalidation_resource_layout",
                                        1,
                                    )
                                }
                                ActionCacheInvalidation::StandardLibrary => {
                                    nia_timing::emit_counter(
                                        "build.action_cache_invalidation_standard_library",
                                        1,
                                    )
                                }
                                ActionCacheInvalidation::BuildProtocol => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_build_protocol",
                                    1,
                                ),
                            }
                        }
                    }
                    ActionCacheMissReason::Corrupt => {
                        nia_timing::emit_counter("build.action_cache_miss_corrupt", 1);
                    }
                    ActionCacheMissReason::ReadError => {
                        nia_timing::emit_counter("build.action_cache_miss_read_error", 1);
                    }
                    ActionCacheMissReason::WriteError => {
                        nia_timing::emit_counter("build.action_cache_miss_write_error", 1);
                    }
                }
            }
        }
    }
}

pub fn resolve_build_invocation(request: BuildRequest) -> Result<BuildInvocation, BuildError> {
    let start = match request.root {
        Some(root) => root,
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let package_root = find_package_root(&start)?;
    let build_script = package_root.join("build.nia");
    let build_dir = package_root.join(".nia-build");
    let runner_dir = build_dir.join("runner");
    let transient_name = next_build_invocation_name();
    Ok(BuildInvocation {
        toolchain: request.toolchain,
        cache_dir: package_root.join(".nia-cache"),
        runner_executable: runner_dir.join(format!("nia-build-runner-{transient_name}")),
        plan_draft: build_dir.join(format!(".build-plan-{transient_name}.draft")),
        plan_path: build_dir.join("build-plan.bin"),
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
        max_parallel_actions: request.max_parallel_actions,
    })
}

static BUILD_INVOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_build_invocation_name() -> String {
    let sequence = BUILD_INVOCATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence}", std::process::id())
}

pub fn build_runner_source(invocation: &BuildInvocation) -> Result<BuildRunnerSource, BuildError> {
    let path = invocation
        .runner_dir
        .join("root.nia")
        .to_string_lossy()
        .into_owned();
    build_runner_source_for_path(invocation, path)
}

fn build_runner_source_for_path(
    invocation: &BuildInvocation,
    path: String,
) -> Result<BuildRunnerSource, BuildError> {
    let _ = invocation;
    let mut source = String::new();
    source.push_str(
        r#"
using std::build;
using std::collections;
using std::fmt;
using std::fs;
using std::io;
using std::mem;
using std::process;
using buildScript;

extend[T] build::Error!T {
    fn reportAndExit(self, init: process::Init) process::ExitCode!T {
        switch self {
            !value => !value,
            error! => {
                let mut buffer: [1024]u8 = [_]u8[0; 1024];
                let mut stderr = io::FileWriter::stderr(init.io(), &mut buffer[..]);
                switch stderr.print(&"build error: {}\n", &[&error]).asBuildError(
                    build::ErrorOperation::Report,
                    build::ErrorSubject::Diagnostic,
                ) {
                    !reported => {
                        _ = reported;
                    },
                    reportError! => {
                        return reportError.asExitCode()!;
                    },
                }
                switch stderr.flush().asBuildError(
                    build::ErrorOperation::Report,
                    build::ErrorSubject::Diagnostic,
                ) {
                    !reported => {
                        _ = reported;
                    },
                    reportError! => {
                        return reportError.asExitCode()!;
                    },
                }
                error.asExitCode()!
            },
        }
    }
}

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

    fn deinit(
        &mut self,
        allocator: &mut mem::Allocator,
        subject: build::ErrorSubject,
    ) build::Error!void {
        let mut firstError: ?build::Error = null;
        rememberTargetCleanupError(&mut firstError, self.endian.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
        rememberTargetCleanupError(&mut firstError, self.abi.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
        rememberTargetCleanupError(&mut firstError, self.env.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
        rememberTargetCleanupError(&mut firstError, self.os.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
        rememberTargetCleanupError(&mut firstError, self.vendor.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
        rememberTargetCleanupError(&mut firstError, self.arch.deinit(allocator).asBuildError(build::ErrorOperation::Release, subject));
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
    subject: build::ErrorSubject,
) build::Error!fs::PathView {
    let arg = switch init.args().get(index) {
        ?value => {
            value
        },
        null => {
            return build::Error::Internal(build::ErrorOperation::Initialize)!;
        },
    };
    fs::PathView::from_utf8_into(allocator, arg.bytes(), storage).asBuildError(
        build::ErrorOperation::Encode,
        subject,
    )
}

fn targetArg(
    init: process::Init,
    allocator: &mut mem::Allocator,
    startIndex: usize,
    storage: &mut TargetText,
    subject: build::ErrorSubject,
) build::Error!build::TargetView {
    let arch = pathArg(init, allocator, startIndex, &mut storage.arch, subject).?.string();
    let vendor = pathArg(init, allocator, startIndex + 1usize, &mut storage.vendor, subject).?.string();
    let os = pathArg(init, allocator, startIndex + 2usize, &mut storage.os, subject).?.string();
    let env = pathArg(init, allocator, startIndex + 3usize, &mut storage.env, subject).?.string();
    let abi = pathArg(init, allocator, startIndex + 4usize, &mut storage.abi, subject).?.string();
    let endian = pathArg(init, allocator, startIndex + 5usize, &mut storage.endian, subject).?.string();
    let pointerWidthArg = switch init.args().get(startIndex + 6usize) {
        ?value => {
            value
        },
        null => {
            return build::Error::Internal(build::ErrorOperation::Initialize)!;
        },
    };
    let pointerWidth = switch fmt::parse[u32](pointerWidthArg) {
        !value => {
            value
        },
        error! => {
            _ = error;
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: subject,
            }!;
        },
    };
    !build::TargetView::init(arch, vendor, os, env, abi, endian, pointerWidth)
}

fn u32Arg(
    init: process::Init,
    index: usize,
    subject: build::ErrorSubject,
) build::Error!u32 {
    let arg = switch init.args().get(index) {
        ?value => {
            value
        },
        null => {
            return build::Error::Internal(build::ErrorOperation::Initialize)!;
        },
    };
    switch fmt::parse[u32](arg) {
        !value => !value,
        error! => {
            _ = error;
            build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: subject,
            }!
        },
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut pageAllocator);
    defer allocator.deinit().ok().exit().?;

    let mut packageRootText = collections::ArrayList[char]::init();
    defer packageRootText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::PackageRoot).reportAndExit(init).?;
    let mut buildDirText = collections::ArrayList[char]::init();
    defer buildDirText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::BuildDir).reportAndExit(init).?;
    let mut cacheDirText = collections::ArrayList[char]::init();
    defer cacheDirText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::CacheDir).reportAndExit(init).?;
    let mut toolchainText = collections::ArrayList[char]::init();
    defer toolchainText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::ToolchainExecutable).reportAndExit(init).?;
    let mut resourceRootText = collections::ArrayList[char]::init();
    defer resourceRootText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::ToolchainResourceRoot).reportAndExit(init).?;

    let packageRoot = pathArg(init, &mut allocator, 1usize, &mut packageRootText, build::ErrorSubject::PackageRoot).reportAndExit(init).?;
    let buildDir = pathArg(init, &mut allocator, 2usize, &mut buildDirText, build::ErrorSubject::BuildDir).reportAndExit(init).?;
    let cacheDir = pathArg(init, &mut allocator, 3usize, &mut cacheDirText, build::ErrorSubject::CacheDir).reportAndExit(init).?;
    let toolchainExecutable = pathArg(init, &mut allocator, 4usize, &mut toolchainText, build::ErrorSubject::ToolchainExecutable).reportAndExit(init).?;
    let toolchainResourceRoot = pathArg(init, &mut allocator, 5usize, &mut resourceRootText, build::ErrorSubject::ToolchainResourceRoot).reportAndExit(init).?;
    let mut hostTargetText = TargetText::init();
    defer hostTargetText.deinit(&mut allocator, build::ErrorSubject::HostTarget).reportAndExit(init).?;
    let hostTarget = targetArg(init, &mut allocator, 6usize, &mut hostTargetText, build::ErrorSubject::HostTarget).reportAndExit(init).?;
    let mut artifactTargetText = TargetText::init();
    defer artifactTargetText.deinit(&mut allocator, build::ErrorSubject::ArtifactTarget).reportAndExit(init).?;
    let artifactTarget = targetArg(init, &mut allocator, 13usize, &mut artifactTargetText, build::ErrorSubject::ArtifactTarget).reportAndExit(init).?;
    let planSchemaVersion = u32Arg(init, 20usize, build::ErrorSubject::BuildPlan).reportAndExit(init).?;
    let mut planDraftText = collections::ArrayList[char]::init();
    defer planDraftText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::BuildPlan).reportAndExit(init).?;
    let planDraft = pathArg(init, &mut allocator, 21usize, &mut planDraftText, build::ErrorSubject::BuildPlan).reportAndExit(init).?;

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
        planSchemaVersion,
        22usize,
"#,
    );
    source.push_str(
        r#"    ).reportAndExit(init).?;
    defer api.deinit().reportAndExit(init).?;

    switch buildScript::build(&mut api) {
        !ok => {
            _ = ok;
        },
        error! => {
            switch api.reportError(error) {
                !reported => {
                    _ = reported;
                },
                reportError! => {
                    return reportError.asExitCode()!;
                },
            }
            return error.asExitCode()!;
        },
    }
    switch api.writePlanDraft(planDraft) {
        !ok => {
            _ = ok;
        },
        error! => {
            switch api.reportError(error) {
                !reported => {
                    _ = reported;
                },
                reportError! => {
                    return reportError.asExitCode()!;
                },
            }
            return error.asExitCode()!;
        },
    }
    !{}
}
"#,
    );
    Ok(BuildRunnerSource { path, source })
}

fn compile_build_runner(invocation: &BuildInvocation) -> Result<PathBuf, BuildError> {
    fs::create_dir_all(&invocation.runner_dir).map_err(|error| {
        BuildError::CreateRunnerDirectory {
            path: invocation.runner_dir.clone(),
            error,
        }
    })?;
    let runner = build_runner_source(invocation)?;
    if let Some(parent) = invocation.runner_executable.parent() {
        fs::create_dir_all(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let driver = Driver::with_config(build_runner_driver_config(invocation));
    driver.set_source(runner.path.clone(), runner.source.clone());
    let output = driver.link_executable(LinkExecutableRequest::new(
        CheckRequest::new(runner.path.clone())
            .with_module_map(build_runner_module_map(invocation))
            .with_timings(invocation.timings),
        &invocation.runner_executable,
    ));
    output.result.map_err(|error| BuildError::CompileRunner {
        path: runner.path,
        source: runner.source,
        error: Box::new(error),
    })?;
    Ok(invocation.runner_executable.clone())
}

fn build_runner_driver_config(invocation: &BuildInvocation) -> DriverConfig {
    DriverConfig {
        artifact_cache_dir: Some(invocation.cache_dir.clone()),
        ..DriverConfig::new(Arc::clone(&invocation.toolchain))
    }
    .with_artifact_target(invocation.toolchain.host_target().clone())
}

fn build_runner_module_map(invocation: &BuildInvocation) -> ModuleMap {
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "buildScript",
        SourcePath::new(invocation.build_script.to_string_lossy().into_owned()),
    );
    module_map
}

fn prepare_build_directories(invocation: &BuildInvocation) -> Result<(), BuildError> {
    fs::create_dir_all(&invocation.build_dir).map_err(|error| {
        BuildError::CreateBuildDirectory {
            path: invocation.build_dir.clone(),
            error,
        }
    })?;
    fs::create_dir_all(&invocation.cache_dir).map_err(|error| BuildError::CreateCacheDirectory {
        path: invocation.cache_dir.clone(),
        error,
    })
}

fn run_build_runner(
    invocation: &BuildInvocation,
    runner_executable: &Path,
) -> Result<BuildPlan, BuildError> {
    match fs::remove_file(&invocation.plan_draft) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(BuildError::PreparePlanDraft {
                path: invocation.plan_draft.clone(),
                error,
            });
        }
    }
    let mut command = Command::new(runner_executable);
    command.current_dir(&invocation.package_root);
    command.args(build_runner_args(invocation));
    let result = match command.status() {
        Ok(status) if status.success() => match read_build_plan(&invocation.plan_draft) {
            Ok(plan) => publish_build_plan(&invocation.plan_path, &plan)
                .map(|()| plan)
                .map_err(|error| BuildError::PublishBuildPlan {
                    path: invocation.plan_path.clone(),
                    error,
                }),
            Err(error) => Err(BuildError::ReadPlanDraft {
                path: invocation.plan_draft.clone(),
                error,
            }),
        },
        Ok(status) => Err(BuildError::RunnerFailed {
            path: runner_executable.to_path_buf(),
            status,
        }),
        Err(error) => Err(BuildError::RunRunner {
            path: runner_executable.to_path_buf(),
            error,
        }),
    };
    let cleanup = match fs::remove_file(&invocation.plan_draft) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BuildError::CleanupPlanDraft {
            path: invocation.plan_draft.clone(),
            error,
        }),
    };
    match result {
        Ok(plan) => cleanup.map(|()| plan),
        Err(error) => Err(error),
    }
}

fn run_and_cleanup_build_runner(
    invocation: &BuildInvocation,
    runner_executable: &Path,
) -> Result<BuildPlan, BuildError> {
    let result = run_build_runner(invocation, runner_executable);
    let cleanup = match fs::remove_file(runner_executable) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BuildError::CleanupRunnerExecutable {
            path: runner_executable.to_path_buf(),
            error,
        }),
    };
    match result {
        Ok(plan) => cleanup.map(|()| plan),
        Err(error) => Err(error),
    }
}

fn build_runner_args(invocation: &BuildInvocation) -> Vec<OsString> {
    let mut args = vec![
        invocation.package_root.as_os_str().to_owned(),
        invocation.build_dir.as_os_str().to_owned(),
        invocation.cache_dir.as_os_str().to_owned(),
        invocation
            .toolchain
            .compiler_executable()
            .as_os_str()
            .to_owned(),
        invocation.toolchain.resource_root().as_os_str().to_owned(),
    ];
    args.extend(target_runner_args(invocation.toolchain.host_target()).map(OsString::from));
    args.extend(target_runner_args(invocation.toolchain.artifact_target()).map(OsString::from));
    args.push(OsString::from(BUILD_PLAN_SCHEMA_VERSION.to_string()));
    args.push(invocation.plan_draft.as_os_str().to_owned());
    if let Some(step) = invocation.step.as_runner_arg() {
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

        let plan =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&child))
                .expect("build invocation");

        assert_eq!(plan.package_root, root);
        assert_eq!(plan.build_script, plan.package_root.join("build.nia"));
        assert!(plan.toolchain.compiler_executable().is_file());
        assert!(plan.toolchain.resource_root().is_dir());
        assert_eq!(plan.build_dir, plan.package_root.join(".nia-build"));
        assert_eq!(plan.cache_dir, plan.package_root.join(".nia-cache"));
        assert_eq!(plan.runner_dir, plan.package_root.join(".nia-build/runner"));
        assert_eq!(
            plan.runner_executable.parent(),
            Some(plan.runner_dir.as_path())
        );
        assert!(
            plan.runner_executable
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("nia-build-runner-")
        );
        assert_eq!(plan.plan_draft.parent(), Some(plan.build_dir.as_path()));
        assert!(
            plan.plan_draft
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".build-plan-")
        );
        assert_eq!(
            plan.plan_path,
            plan.package_root.join(".nia-build/build-plan.bin")
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
    }

    #[test]
    fn concurrent_invocations_use_disjoint_transient_paths() {
        let root = temp_root("concurrent_invocations_use_disjoint_transient_paths");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let first =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("first invocation");
        let second =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("second invocation");

        assert_ne!(first.runner_executable, second.runner_executable);
        assert_ne!(first.plan_draft, second.plan_draft);
        assert_eq!(first.plan_path, second.plan_path);
        assert_eq!(first.build_dir, second.build_dir);
        assert_eq!(first.cache_dir, second.cache_dir);
    }

    #[test]
    fn generated_runner_invokes_build_script_as_normal_nia_module() {
        let root = temp_root("generated_runner_invokes_build_script_as_normal_nia_module");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");

        let runner = build_runner_source(&plan).expect("build runner source");

        assert_eq!(
            runner.path,
            plan.runner_dir.join("root.nia").to_string_lossy()
        );
        assert!(runner.source.contains("using std::build;"));
        assert!(runner.source.contains("using std::fs;"));
        assert!(runner.source.contains("using std::io;"));
        assert!(runner.source.contains("using std::mem;"));
        assert!(runner.source.contains("using buildScript;"));
        assert!(runner.source.contains("fn pathArg("));
        assert!(runner.source.contains("fn targetArg("));
        assert!(runner.source.contains("fn reportAndExit("));
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
        assert!(
            runner
                .source
                .contains("targetArg(init, &mut allocator, 6usize, &mut hostTargetText, build::ErrorSubject::HostTarget)")
        );
        assert!(
            runner
                .source
                .contains("targetArg(init, &mut allocator, 13usize, &mut artifactTargetText, build::ErrorSubject::ArtifactTarget)")
        );
        assert!(runner.source.contains("hostTarget,"));
        assert!(runner.source.contains("artifactTarget,"));
        assert!(
            runner
                .source
                .contains("let planSchemaVersion = u32Arg(init, 20usize")
        );
        assert!(
            runner
                .source
                .contains("pathArg(init, &mut allocator, 21usize")
        );
        assert!(runner.source.contains("planSchemaVersion,"));
        assert!(runner.source.contains("22usize,"));
        assert!(runner.source.contains("api.writePlanDraft(planDraft)"));
        assert!(runner.source.contains("api.reportError(error)"));
        assert!(runner.source.contains(").reportAndExit(init).?;"));
        assert!(!runner.source.contains("runRequestedStep"));
        assert!(!runner.source.contains("reportActions"));
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
        let plan = resolve_build_invocation(
            BuildRequest::new(Arc::clone(&toolchain))
                .with_root(&root)
                .with_step("inspect"),
        )
        .expect("build invocation");

        let config = build_runner_driver_config(&plan);
        let args = build_runner_args(&plan);

        assert_eq!(config.artifact_target, *toolchain.host_target());
        assert_ne!(config.artifact_target, *toolchain.artifact_target());
        assert_eq!(args.len(), 22);
        assert_eq!(args[5], OsString::from(&toolchain.host_target().arch));
        assert_eq!(
            args[11],
            OsString::from(toolchain.host_target().pointer_width.to_string())
        );
        assert_eq!(args[12], OsString::from("artifact-arch"));
        assert_eq!(args[13], OsString::from("artifact-vendor"));
        assert_eq!(args[14], OsString::from("artifact-os"));
        assert_eq!(args[15], OsString::from("artifact-env"));
        assert_eq!(args[16], OsString::from("artifact-abi"));
        assert_eq!(args[17], OsString::from("big"));
        assert_eq!(args[18], OsString::from("32"));
        assert_eq!(
            args[19],
            OsString::from(BUILD_PLAN_SCHEMA_VERSION.to_string())
        );
        assert_eq!(args[20], plan.plan_draft.as_os_str());
        assert_eq!(args[21], OsString::from("inspect"));
    }

    #[test]
    fn preserves_named_step() {
        let root = temp_root("preserves_named_step");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_step("install"),
        )
        .expect("build invocation");

        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
        assert_eq!(plan.step.as_runner_arg(), Some("install"));
    }

    #[test]
    fn preserves_build_action_parallelism_limit() {
        let root = temp_root("preserves_build_action_parallelism_limit");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let limit = NonZeroUsize::new(3).unwrap();

        let invocation = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_max_parallel_actions(limit),
        )
        .expect("build invocation");

        assert_eq!(invocation.max_parallel_actions, Some(limit));
    }

    #[test]
    fn reports_missing_build_script_from_start_directory() {
        let root = temp_root("reports_missing_build_script_from_start_directory");

        let error =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect_err("missing build script");

        assert!(matches!(error, BuildError::MissingBuildScript { start } if start == root));
    }

    #[test]
    fn prepares_build_and_cache_directories() {
        let root = temp_root("prepares_build_and_cache_directories");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let plan =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");

        prepare_build_directories(&plan).expect("prepare build directories");

        assert!(plan.build_dir.is_dir());
        assert!(plan.cache_dir.is_dir());
    }

    #[test]
    fn generated_runner_source_does_not_embed_package_paths() {
        let root = temp_root("generated_runner_source_does_not_embed_package_paths");
        let package_root = root.join("quote\"slash\\tab\t");
        std::fs::create_dir_all(&package_root).expect("create package root");
        std::fs::write(package_root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout()).with_root(&package_root),
        )
        .expect("build invocation");
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
