// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fmt, fs, io,
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, LinkExecutableRequest, TimingMode,
};
use nia_imports::ModuleMap;
use nia_source::SourcePath;
use nia_timing::{TimingFormat, TimingOptions};

const RUNNER_OUTPUT_TAIL_BYTES: usize = 64 * 1024;

mod action_cache;
mod coordinator;
mod lock;
mod output_recovery;
mod plan;
mod process_output;
mod resources;
mod runner_config;

use process_output::{
    CapturedStream, StreamCapture, capture_stream, prepare_process_group,
    terminate_process_descendants, terminate_process_tree,
};

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
    pub optimization: OptimizationMode,
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
            optimization: OptimizationMode::O0,
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

    pub fn with_optimization(mut self, optimization: OptimizationMode) -> Self {
        self.optimization = optimization;
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
    pub runner_config: PathBuf,
    pub plan_draft: PathBuf,
    pub plan_path: PathBuf,
    pub step: BuildStepSelection,
    pub timings: TimingMode,
    pub timing_format: TimingFormat,
    pub max_parallel_actions: Option<NonZeroUsize>,
    pub optimization: OptimizationMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStepSelection {
    Default,
    Named(String),
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
    PrepareRunnerConfiguration {
        path: PathBuf,
        error: io::Error,
    },
    CleanupRunnerConfiguration {
        path: PathBuf,
        error: io::Error,
    },
    RunnerConfigurationFieldTooLarge {
        role: &'static str,
        len: usize,
    },
    RunnerConfigurationTooLarge {
        len: usize,
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
        stdout: Vec<u8>,
        stderr: Vec<u8>,
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
            Self::PrepareRunnerConfiguration { path, error } => write!(
                f,
                "failed to prepare build-runner configuration `{}`: {error}",
                path.display()
            ),
            Self::CleanupRunnerConfiguration { path, error } => write!(
                f,
                "failed to remove build-runner configuration `{}`: {error}",
                path.display()
            ),
            Self::RunnerConfigurationFieldTooLarge { role, len } => write!(
                f,
                "failed to encode build-runner configuration: {role} is {len} bytes",
            ),
            Self::RunnerConfigurationTooLarge { len } => write!(
                f,
                "failed to encode build-runner configuration: payload is {len} bytes",
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
            Self::RunnerFailed {
                path,
                status,
                stdout,
                stderr,
            } => {
                write!(
                    f,
                    "build runner `{}` exited with status {status}",
                    path.display()
                )?;
                write_runner_output(f, "stdout", stdout)?;
                write_runner_output(f, "stderr", stderr)
            }
            Self::MissingBuildScript { start } => write!(
                f,
                "failed to find `build.nia` from `{}` or any parent directory",
                start.display()
            ),
        }
    }
}

fn write_runner_output(f: &mut fmt::Formatter<'_>, stream: &str, bytes: &[u8]) -> fmt::Result {
    if bytes.is_empty() {
        return Ok(());
    }
    write!(f, "\nrunner {stream} (last {} bytes):\n", bytes.len())?;
    f.write_str(&String::from_utf8_lossy(bytes))
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
                                ActionCacheInvalidation::Command => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_command",
                                    1,
                                ),
                                ActionCacheInvalidation::ExternalTool => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_external_tool",
                                    1,
                                ),
                                ActionCacheInvalidation::Environment => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_environment",
                                    1,
                                ),
                                ActionCacheInvalidation::Inputs => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_inputs",
                                    1,
                                ),
                                ActionCacheInvalidation::Dependencies => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_dependencies",
                                    1,
                                ),
                                ActionCacheInvalidation::WorkingDirectory => {
                                    nia_timing::emit_counter(
                                        "build.action_cache_invalidation_working_directory",
                                        1,
                                    )
                                }
                                ActionCacheInvalidation::PackageRoots => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_package_roots",
                                    1,
                                ),
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
        runner_config: build_dir.join(format!(".build-runner-{transient_name}.config")),
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
        optimization: request.optimization,
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
using std::fmt;
using std::fs;
using std::io;
using std::mem;
using std::process;
using std::string;
using buildScript;

extend[T] build::Error!T {
    fn reportAndExit(self, init: process::Init) process::ExitCode!T {
        switch self {
            !value => !value,
            error! => {
                let mut buffer: [1024]u8 = [_]u8[0; 1024];
                let mut stderr = io::FileWriter::stderr(&mut buffer[..]);
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
    arch: string::String,
    vendor: string::String,
    os: string::String,
    env: string::String,
    abi: string::String,
    endian: string::String,
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
            arch: string::String::init(),
            vendor: string::String::init(),
            os: string::String::init(),
            env: string::String::init(),
            abi: string::String::init(),
            endian: string::String::init(),
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

struct ConfigCursor {
    bytes: &[u8],
    position: usize,
}

extend ConfigCursor {
    fn init(bytes: &[u8]) ConfigCursor {
        { bytes: bytes, position: 0 }
    }

    fn remaining(&self) usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, count: usize) build::Error!&[u8] {
        if count > self.remaining() {
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::RunnerConfiguration,
            }!;
        }
        let start = self.position;
        self.position += count;
        !&self.bytes[start..self.position]
    }

    fn byte(&mut self) build::Error!u8 {
        !self.take(1).?[0]
    }

    fn u32(&mut self) build::Error!u32 {
        let bytes = self.take(4).?;
        !((bytes[0] as u32)
            | ((bytes[1] as u32) << 8u32)
            | ((bytes[2] as u32) << 16u32)
            | ((bytes[3] as u32) << 24u32))
    }

    fn u64(&mut self) build::Error!u64 {
        let low = self.u32().? as u64;
        let high = self.u32().? as u64;
        !(low | (high << 32u64))
    }

    fn textBytes(&mut self) build::Error!&[u8] {
        let len = self.u32().? as usize;
        self.take(len)
    }
}

fn runnerConfigChecksum(bytes: &[u8]) u64 {
    let mut first = 1u32;
    let mut second = 0u32;
    for &byte in bytes {
        first = (first + (byte as u32)) % 65521u32;
        second = (second + first) % 65521u32;
    }
    ((second as u64) << 32u64) | (first as u64)
}

fn readText(
    cursor: &mut ConfigCursor,
    allocator: &mut mem::Allocator,
    storage: &mut string::String,
    subject: build::ErrorSubject,
) build::Error!&[char] {
    storage.* = switch string::String::fromUtf8(allocator, cursor.textBytes().?) {
        !text => text,
        string::TextError::InvalidUtf8(error)! => {
            _ = error;
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: subject,
            }!;
        },
        string::TextError::Allocation(error)! => {
            return build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: subject,
                cause: build::ErrorCause::Memory(error),
            }!;
        },
    };
    !storage.text()
}

fn readPath(
    cursor: &mut ConfigCursor,
    allocator: &mut mem::Allocator,
    storage: &mut fs::PathBuf,
    subject: build::ErrorSubject,
) build::Error!fs::PathView {
    storage.* = switch fs::PathBuf::fromUtf8(allocator, cursor.textBytes().?) {
        !path => path,
        string::TextError::InvalidUtf8(error)! => {
            _ = error;
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: subject,
            }!;
        },
        string::TextError::Allocation(error)! => {
            return build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: subject,
                cause: build::ErrorCause::Memory(error),
            }!;
        },
    };
    !storage.view()
}

fn readTarget(
    cursor: &mut ConfigCursor,
    allocator: &mut mem::Allocator,
    storage: &mut TargetText,
    subject: build::ErrorSubject,
) build::Error!build::TargetView {
    let arch = readText(cursor, allocator, &mut storage.arch, subject).?;
    let vendor = readText(cursor, allocator, &mut storage.vendor, subject).?;
    let os = readText(cursor, allocator, &mut storage.os, subject).?;
    let env = readText(cursor, allocator, &mut storage.env, subject).?;
    let abi = readText(cursor, allocator, &mut storage.abi, subject).?;
    let endian = readText(cursor, allocator, &mut storage.endian, subject).?;
    let pointerWidth = cursor.u32().?;
    !build::TargetView::init(arch, vendor, os, env, abi, endian, pointerWidth)
}

fn readOptimization(cursor: &mut ConfigCursor) build::Error!build::OptimizationMode {
    let value = cursor.u32().?;
    switch value {
        0 => !build::OptimizationMode::O0,
        1 => !build::OptimizationMode::O1,
        2 => !build::OptimizationMode::O2,
        3 => !build::OptimizationMode::O3,
        4 => !build::OptimizationMode::Os,
        5 => !build::OptimizationMode::Oz,
        _ => build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::BuildPlan,
        }!,
    }
}

fn configPathArg(
    init: process::Init,
    allocator: &mut mem::Allocator,
    storage: &mut fs::PathBuf,
) build::Error!fs::PathView {
    if init.args().len() != 3 {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }!;
    }
    let flag = switch init.args().get(1) {
        ?value => value,
        null => return build::Error::Internal(build::ErrorOperation::Initialize)!,
    };
    if not flag.bytes().equals(&b"--config") {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }!;
    }
    let arg = switch init.args().get(2) {
        ?value => value,
        null => return build::Error::Internal(build::ErrorOperation::Initialize)!,
    };
    storage.* = switch fs::PathBuf::fromUtf8(allocator, arg.bytes()) {
        !path => path,
        string::TextError::InvalidUtf8(error)! => {
            _ = error;
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::RunnerConfiguration,
            }!;
        },
        string::TextError::Allocation(error)! => {
            return build::Error::Failure {
                operation: build::ErrorOperation::Retain,
                subject: build::ErrorSubject::RunnerConfiguration,
                cause: build::ErrorCause::Memory(error),
            }!;
        },
    };
    !storage.view()
}

pub fn main(init: process::Init) process::ExitCode!void {
    let mut pageAllocator = mem::PageAllocator::init();
    let mut allocator = mem::GeneralPurposeAllocator::init(&mut pageAllocator);
    defer allocator.deinit().ok().exit().?;

    let mut configPathStorage = fs::PathBuf::init();
    defer configPathStorage.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::RunnerConfiguration).reportAndExit(init).?;
    let configPath = configPathArg(init, &mut allocator, &mut configPathStorage).reportAndExit(init).?;
    let mut configFile = fs::File::open(configPath, fs::OpenOptions::readOnly()).asBuildError(
        build::ErrorOperation::Initialize,
        build::ErrorSubject::RunnerConfiguration,
    ).reportAndExit(init).?;
    defer configFile.close().asBuildError(build::ErrorOperation::Release, build::ErrorSubject::RunnerConfiguration).reportAndExit(init).?;
    let configLen64 = configFile.len().asBuildError(
        build::ErrorOperation::Initialize,
        build::ErrorSubject::RunnerConfiguration,
    ).reportAndExit(init).?;
    if configLen64 < 24u64 or configLen64 > __NIA_RUNNER_CONFIG_MAX_BYTES__u64 {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }.asExitCode()!;
    }
    let configLen = configLen64 as usize;
    let configBytes = allocator.allocSlice[u8](configLen).asBuildError(
        build::ErrorOperation::Retain,
        build::ErrorSubject::RunnerConfiguration,
    ).reportAndExit(init).?;
    defer allocator.freeSlice(configBytes).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::RunnerConfiguration).reportAndExit(init).?;
    let mut readBuffer: [4096]u8 = [_]u8[0; 4096];
    let mut configReader = configFile.reader(&mut readBuffer).asBuildError(
        build::ErrorOperation::Initialize,
        build::ErrorSubject::RunnerConfiguration,
    ).reportAndExit(init).?;
    configReader.readExact(configBytes).asBuildError(
        build::ErrorOperation::Initialize,
        build::ErrorSubject::RunnerConfiguration,
    ).reportAndExit(init).?;

    let mut envelope = ConfigCursor::init(configBytes);
    if not envelope.take(8).reportAndExit(init).?.equals(&b"__NIA_RUNNER_CONFIG_MAGIC__")
        or envelope.u32().reportAndExit(init).? != __NIA_RUNNER_CONFIG_SCHEMA_VERSION__u32
    {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }.asExitCode()!;
    }
    let payloadLen = envelope.u32().reportAndExit(init).? as usize;
    let expectedChecksum = envelope.u64().reportAndExit(init).?;
    if payloadLen != envelope.remaining() {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }.asExitCode()!;
    }
    let payload = envelope.take(payloadLen).reportAndExit(init).?;
    if expectedChecksum != runnerConfigChecksum(payload) {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }.asExitCode()!;
    }
    let mut config = ConfigCursor::init(payload);

    let mut packageRootPath = fs::PathBuf::init();
    defer packageRootPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::PackageRoot).reportAndExit(init).?;
    let mut buildDirPath = fs::PathBuf::init();
    defer buildDirPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::BuildDir).reportAndExit(init).?;
    let mut cacheDirPath = fs::PathBuf::init();
    defer cacheDirPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::CacheDir).reportAndExit(init).?;
    let mut toolchainPath = fs::PathBuf::init();
    defer toolchainPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::ToolchainExecutable).reportAndExit(init).?;
    let mut resourceRootPath = fs::PathBuf::init();
    defer resourceRootPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::ToolchainResourceRoot).reportAndExit(init).?;

    let packageRoot = readPath(&mut config, &mut allocator, &mut packageRootPath, build::ErrorSubject::PackageRoot).reportAndExit(init).?;
    let buildDir = readPath(&mut config, &mut allocator, &mut buildDirPath, build::ErrorSubject::BuildDir).reportAndExit(init).?;
    let cacheDir = readPath(&mut config, &mut allocator, &mut cacheDirPath, build::ErrorSubject::CacheDir).reportAndExit(init).?;
    let toolchainExecutable = readPath(&mut config, &mut allocator, &mut toolchainPath, build::ErrorSubject::ToolchainExecutable).reportAndExit(init).?;
    let toolchainResourceRoot = readPath(&mut config, &mut allocator, &mut resourceRootPath, build::ErrorSubject::ToolchainResourceRoot).reportAndExit(init).?;
    let mut hostTargetText = TargetText::init();
    defer hostTargetText.deinit(&mut allocator, build::ErrorSubject::HostTarget).reportAndExit(init).?;
    let hostTarget = readTarget(&mut config, &mut allocator, &mut hostTargetText, build::ErrorSubject::HostTarget).reportAndExit(init).?;
    let mut artifactTargetText = TargetText::init();
    defer artifactTargetText.deinit(&mut allocator, build::ErrorSubject::ArtifactTarget).reportAndExit(init).?;
    let artifactTarget = readTarget(&mut config, &mut allocator, &mut artifactTargetText, build::ErrorSubject::ArtifactTarget).reportAndExit(init).?;
    let defaultOptimization = readOptimization(&mut config).reportAndExit(init).?;
    let planSchemaVersion = config.u32().reportAndExit(init).?;
    let mut planDraftPath = fs::PathBuf::init();
    defer planDraftPath.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::BuildPlan).reportAndExit(init).?;
    let planDraft = readPath(&mut config, &mut allocator, &mut planDraftPath, build::ErrorSubject::BuildPlan).reportAndExit(init).?;
    let mut requestedStepText = string::String::init();
    defer requestedStepText.deinit(&mut allocator).asBuildError(build::ErrorOperation::Release, build::ErrorSubject::RequestedStep).reportAndExit(init).?;
    let requestedStep: ?&[char] = switch config.byte().reportAndExit(init).? {
        0 => null,
        1 => ?readText(
            &mut config,
            &mut allocator,
            &mut requestedStepText,
            build::ErrorSubject::RequestedStep,
        ).reportAndExit(init).?,
        _ => {
            return build::Error::Invalid {
                operation: build::ErrorOperation::Validate,
                subject: build::ErrorSubject::RunnerConfiguration,
            }.asExitCode()!;
        },
    };
    if config.remaining() != 0 {
        return build::Error::Invalid {
            operation: build::ErrorOperation::Validate,
            subject: build::ErrorSubject::RunnerConfiguration,
        }.asExitCode()!;
    }

    let mut api = build::Build::init(
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
        defaultOptimization,
        planSchemaVersion,
        requestedStep,
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
    let source = source
        .replace(
            "__NIA_RUNNER_CONFIG_SCHEMA_VERSION__",
            &runner_config::RUNNER_CONFIG_SCHEMA_VERSION.to_string(),
        )
        .replace(
            "__NIA_RUNNER_CONFIG_MAX_BYTES__",
            &runner_config::RUNNER_CONFIG_MAX_BYTES.to_string(),
        )
        .replace(
            "__NIA_RUNNER_CONFIG_MAGIC__",
            runner_config::RUNNER_CONFIG_MAGIC_TEXT,
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
    prepare_runner_configuration(invocation)?;
    let mut command = Command::new(runner_executable);
    command
        .current_dir(&invocation.package_root)
        .arg("--config")
        .arg(&invocation.runner_config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_process_group(&mut command);
    let result = match execute_runner_process(&mut command) {
        Ok(output) if output.status.success() => match read_build_plan(&invocation.plan_draft) {
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
        Ok(output) => Err(BuildError::RunnerFailed {
            path: runner_executable.to_path_buf(),
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        }),
        Err(error) => Err(BuildError::RunRunner {
            path: runner_executable.to_path_buf(),
            error,
        }),
    };
    let plan_cleanup = match fs::remove_file(&invocation.plan_draft) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BuildError::CleanupPlanDraft {
            path: invocation.plan_draft.clone(),
            error,
        }),
    };
    let config_cleanup = match fs::remove_file(&invocation.runner_config) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BuildError::CleanupRunnerConfiguration {
            path: invocation.runner_config.clone(),
            error,
        }),
    };
    let cleanup = plan_cleanup.and(config_cleanup);
    match result {
        Ok(plan) => cleanup.map(|()| plan),
        Err(error) => Err(error),
    }
}

struct RunnerProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn execute_runner_process(command: &mut Command) -> io::Result<RunnerProcessOutput> {
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_runner(&mut child);
        io::Error::other("build runner stdout pipe was not created")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_runner(&mut child);
        io::Error::other("build runner stderr pipe was not created")
    })?;
    let stdout_reader = match spawn_runner_capture(stdout, CapturedStream::Stdout, "stdout") {
        Ok(reader) => reader,
        Err(error) => {
            terminate_runner(&mut child);
            return Err(error);
        }
    };
    let stderr_reader = match spawn_runner_capture(stderr, CapturedStream::Stderr, "stderr") {
        Ok(reader) => reader,
        Err(error) => {
            terminate_runner(&mut child);
            let _ = stdout_reader.join();
            return Err(error);
        }
    };
    let status = match child.wait() {
        Ok(status) => {
            terminate_process_descendants(child.id());
            status
        }
        Err(error) => {
            terminate_runner(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(error);
        }
    };
    let stdout = join_runner_capture(stdout_reader, "stdout")?;
    let stderr = join_runner_capture(stderr_reader, "stderr")?;
    if let Some(error) = stdout.error {
        return Err(error);
    }
    if let Some(error) = stderr.error {
        return Err(error);
    }
    Ok(RunnerProcessOutput {
        status,
        stdout: stdout.tail,
        stderr: stderr.tail,
    })
}

fn spawn_runner_capture(
    reader: impl io::Read + Send + 'static,
    stream: CapturedStream,
    name: &'static str,
) -> io::Result<thread::JoinHandle<StreamCapture>> {
    thread::Builder::new()
        .name(format!("nia-build-runner-{name}"))
        .spawn(move || capture_stream(reader, stream, true, RUNNER_OUTPUT_TAIL_BYTES))
}

fn join_runner_capture(
    reader: thread::JoinHandle<StreamCapture>,
    stream: &'static str,
) -> io::Result<StreamCapture> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("build runner {stream} capture thread panicked")))
}

fn terminate_runner(child: &mut std::process::Child) {
    terminate_process_tree(child);
    let _ = child.wait();
}

fn prepare_runner_configuration(invocation: &BuildInvocation) -> Result<(), BuildError> {
    let encoded = runner_config::encode(invocation)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&invocation.runner_config)
        .map_err(|error| BuildError::PrepareRunnerConfiguration {
            path: invocation.runner_config.clone(),
            error,
        })?;
    let result = file.write_all(&encoded).and_then(|()| file.sync_all());
    if let Err(error) = result {
        drop(file);
        return match fs::remove_file(&invocation.runner_config) {
            Ok(()) => Err(BuildError::PrepareRunnerConfiguration {
                path: invocation.runner_config.clone(),
                error,
            }),
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => {
                Err(BuildError::PrepareRunnerConfiguration {
                    path: invocation.runner_config.clone(),
                    error,
                })
            }
            Err(error) => Err(BuildError::CleanupRunnerConfiguration {
                path: invocation.runner_config.clone(),
                error,
            }),
        };
    }
    Ok(())
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

    pub(crate) fn test_toolchain_layout_for(
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
        assert_eq!(plan.runner_config.parent(), Some(plan.build_dir.as_path()));
        assert!(
            plan.runner_config
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".build-runner-")
        );
        assert_eq!(
            plan.plan_path,
            plan.package_root.join(".nia-build/build-plan.bin")
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
        assert_eq!(plan.optimization, OptimizationMode::O0);
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
        assert_ne!(first.runner_config, second.runner_config);
        assert_ne!(first.plan_draft, second.plan_draft);
        assert_eq!(first.plan_path, second.plan_path);
        assert_eq!(first.build_dir, second.build_dir);
        assert_eq!(first.cache_dir, second.cache_dir);
    }

    #[test]
    fn runner_configuration_publication_is_exclusive_and_preserves_collisions() {
        let root = temp_root("runner_configuration_publication_is_exclusive");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");
        prepare_build_directories(&invocation).expect("prepare build directories");

        prepare_runner_configuration(&invocation).expect("publish runner configuration");
        let expected = runner_config::encode(&invocation).expect("encode expected configuration");
        assert_eq!(
            std::fs::read(&invocation.runner_config).expect("read runner configuration"),
            expected
        );
        let error = prepare_runner_configuration(&invocation)
            .expect_err("runner configuration collision must fail");
        assert!(matches!(
            error,
            BuildError::PrepareRunnerConfiguration { error, .. }
                if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            std::fs::read(&invocation.runner_config).expect("read preserved configuration"),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_runner_retains_output_context_and_cleans_transients() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("failed_runner_retains_output_context");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");
        prepare_build_directories(&invocation).expect("prepare build directories");
        std::fs::create_dir_all(&invocation.runner_dir).expect("prepare runner directory");
        std::fs::write(
            &invocation.runner_executable,
            "#!/bin/sh\nprintf 'runner stdout\n'\nprintf 'runner stderr\n' >&2\nexit 7\n",
        )
        .expect("write fake runner");
        std::fs::set_permissions(
            &invocation.runner_executable,
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("make fake runner executable");

        let error = run_and_cleanup_build_runner(&invocation, &invocation.runner_executable)
            .expect_err("fake runner must fail");
        match error {
            BuildError::RunnerFailed {
                status,
                stdout,
                stderr,
                ..
            } => {
                assert_eq!(status.code(), Some(7));
                assert_eq!(stdout, b"runner stdout\n");
                assert_eq!(stderr, b"runner stderr\n");
            }
            other => panic!("unexpected runner error: {other:?}"),
        }
        assert!(!invocation.runner_config.exists());
        assert!(!invocation.plan_draft.exists());
        assert!(!invocation.runner_executable.exists());
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
        assert!(runner.source.contains("using std::string;"));
        assert!(runner.source.contains("using buildScript;"));
        assert!(runner.source.contains("struct ConfigCursor"));
        assert!(runner.source.contains("fn readPath("));
        assert!(runner.source.contains("fn readText("));
        assert!(runner.source.contains("fn readTarget("));
        assert!(runner.source.contains("fn readOptimization("));
        assert!(runner.source.contains("runnerConfigChecksum(payload)"));
        assert!(runner.source.contains(&format!(
            "envelope.u32().reportAndExit(init).? != {}u32",
            runner_config::RUNNER_CONFIG_SCHEMA_VERSION
        )));
        assert!(runner.source.contains(&format!(
            "configLen64 > {}u64",
            runner_config::RUNNER_CONFIG_MAX_BYTES
        )));
        assert!(
            runner
                .source
                .contains(std::str::from_utf8(runner_config::RUNNER_CONFIG_MAGIC).unwrap())
        );
        assert!(runner.source.contains("init.args().len() != 3"));
        assert!(runner.source.contains("equals(&b\"--config\")"));
        assert!(runner.source.contains("fn reportAndExit("));
        assert!(runner.source.contains("fs::PathBuf::fromUtf8("));
        assert!(runner.source.contains("string::String::fromUtf8("));
        assert!(runner.source.contains("let mut api = build::Build::init("));
        assert!(runner.source.contains("readPath(&mut config"));
        assert!(runner.source.contains("readTarget(&mut config"));
        assert!(runner.source.contains("hostTarget,"));
        assert!(runner.source.contains("artifactTarget,"));
        assert!(
            runner
                .source
                .contains("let defaultOptimization = readOptimization(&mut config)")
        );
        assert!(
            runner
                .source
                .contains("let planSchemaVersion = config.u32()")
        );
        assert!(runner.source.contains("defaultOptimization,"));
        assert!(runner.source.contains("planSchemaVersion,"));
        assert!(runner.source.contains("        requestedStep,"));
        assert!(!runner.source.contains("pathArg("));
        assert!(!runner.source.contains("stepArgIndex"));
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
                .with_step("inspect")
                .with_optimization(OptimizationMode::Oz),
        )
        .expect("build invocation");

        let config = build_runner_driver_config(&plan);
        let encoded = runner_config::encode(&plan).expect("encode runner configuration");

        assert_eq!(plan.optimization, OptimizationMode::Oz);
        assert_eq!(config.artifact_target, *toolchain.host_target());
        assert_ne!(config.artifact_target, *toolchain.artifact_target());
        assert_eq!(&encoded[..8], b"NIARUNCF");
        assert_eq!(
            u32::from_le_bytes(encoded[8..12].try_into().unwrap()),
            runner_config::RUNNER_CONFIG_SCHEMA_VERSION
        );
        assert!(
            encoded
                .windows("artifact-arch".len())
                .any(|value| value == b"artifact-arch")
        );
        assert!(
            encoded
                .windows("inspect".len())
                .any(|value| value == b"inspect")
        );
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
        assert_eq!(plan.step, BuildStepSelection::Named("install".to_string()));
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
