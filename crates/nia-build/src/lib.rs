// SPDX-License-Identifier: GPL-3.0-or-later
//! Bootstrap and invocation owner for `nia build`.
//!
//! This crate discovers `build.nia`, creates invocation-private runner state,
//! compiles and runs the script for the host, decodes its immutable plan, and
//! hands the selected closure to the coordinator. The runner only configures a
//! graph through `std::build`; action execution remains in this process.
//!
//! See the crate README for the source ownership and persistence boundaries.
//!
//! Text diagnostics forward child-process output live. JSON mode disables live
//! forwarding and keeps bounded captured output in structured failure notes.
//! A failed action stops the build unless `--keep-going` lets independent
//! actions finish; failures are then reported together, and a failed test
//! suite keeps its exit or timeout reason and captured streams, prefixed with
//! the suite name. Windows process cleanup
//! produces no terminal output.

use std::{
    collections::{BTreeMap, BTreeSet},
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

use nia_compat::formats::RUNNER_CONFIG;
use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, EmitObjectRequest, LinkExecutableRequest,
    LinkTimeOptimization, ProgramDiagnostic, TimingMode,
};
use nia_imports::ModuleMap;
use nia_package_metadata::DependencyScope;
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};
use nia_source::{SourceIdentity, SourcePath};
use nia_target_config::BuildProfile;
use nia_timing::{TimingFormat, TimingOptions};

const RUNNER_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
const REPORT_ENVIRONMENT_VALUE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.report.environment-value");

fn stream_chunk_len(remaining: u64, buffer_len: usize) -> usize {
    usize::try_from(remaining).map_or(buffer_len, |remaining| remaining.min(buffer_len))
}

mod action_cache;
mod configure_cache;
mod configure_observation;
mod coordinator;
mod lock;
mod output_recovery;
mod process_output;
mod program_search;
mod progress;
mod report;
mod resources;
mod runner_cache;
mod runner_config;
mod runner_observation;

use nia_compat::{create_real_directory, is_link_or_reparse_point};

use process_output::{
    CaptureFailure, CapturedStream, StreamCapture, capture_stream, join_capture_tail,
    join_captures, prepare_process_group, terminate_process_descendants, terminate_process_tree,
};

pub use action_cache::{
    ActionCacheInvalidation, ActionCacheMissReason, ActionCacheOutcome, ActionCacheReport,
};
pub use configure_cache::{
    ConfigureCache, ConfigureCacheCandidate, ConfigureCacheLookup, ConfigureCacheMissReason,
    ConfigureCacheStatus, ConfigureIdentity, ConfigureRecord, invocation_identity, plan_identity,
};
pub use configure_observation::{
    ConfigureManifest, ConfigureObservation, ObservationCodecError, ObservationError,
    ObservationKind, ObservationPolicy, ObservationValidation, observe_directory_contents,
    observe_directory_metadata, observe_directory_presence, observe_environment,
    observe_file_contents, observe_file_presence, observe_into, observe_metadata,
};
pub use coordinator::*;
pub use nia_build_plan::*;
pub use nia_package_metadata::{LockPolicy, LockUpdate, LockUpdateKind, PackageManifest};
pub use output_recovery::OutputRecoveryError;
pub use process_output::{ProcessOutputStream, ProcessOutputWriter};
pub use progress::{
    ProgressActionDescription, ProgressActivity, ProgressEvent, ProgressEventKind,
    ProgressObserver, ProgressOutcome, ProgressPhase, ProgressWaitKind, ProgressWaitOutcome,
};
use progress::{ProgressBuildGuard, ProgressSession};
pub use report::{render_build_error, render_build_error_json};
pub(crate) use runner_observation::decode_and_validate_runner_observations;

/// Inputs and policy knobs for resolving and running one build invocation.
#[derive(Clone)]
pub struct BuildRequest {
    /// Toolchain resources used by the host runner and compiler actions.
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    /// Optional package or workspace root. `None` searches from the current directory.
    pub root: Option<PathBuf>,
    /// Package names selected from a workspace. Repeated selectors are additive.
    pub packages: Vec<String>,
    /// Selects all members of the discovered workspace.
    pub workspace_all: bool,
    /// How dependency resolution may change `nia.lock` and the package cache.
    pub lock_policy: nia_package_metadata::LockPolicy,
    /// Optional named build step; `None` selects the plan default.
    pub step: Option<String>,
    /// Whether compiler timing information is emitted.
    pub timings: TimingMode,
    /// Format used when timing information is enabled.
    pub timing_format: TimingFormat,
    /// Optional upper bound on concurrently ready build actions.
    pub max_parallel_actions: Option<NonZeroUsize>,
    /// Optimization mode passed to compiler actions.
    pub optimization: OptimizationMode,
    /// Profile used for profile-conditional source selection.
    pub profile: BuildProfile,
    /// Cross-module optimization used for final executable products.
    pub link_time_optimization: LinkTimeOptimization,
    /// List the plan's steps instead of running one.
    pub list: bool,
    /// Keep running actions that do not depend on a failed one.
    pub keep_going: bool,
    /// Freeze and validate the plan without executing coordinator actions.
    pub dry_run: bool,
    /// Print the selected graph and cache explanation after configuration.
    pub explain: bool,
    /// Print the canonical plan summary after configuration.
    pub print_plan: bool,
    /// Print the typed configuration observations returned by the runner.
    pub print_observations: bool,
    /// Print cache decisions after configuration and action execution.
    pub print_cache: bool,
    /// Render build inspection output as stable JSON.
    pub output_json: bool,
    /// Forward child streams while retaining bounded tails for failure reports.
    pub forward_output: bool,
    /// Optional observer for invocation-local build progress events.
    pub progress_observer: Option<Arc<dyn ProgressObserver>>,
    /// Optional coordinator for forwarded child output.
    pub process_output_writer: Option<Arc<dyn ProcessOutputWriter>>,
}

impl BuildRequest {
    /// Creates a request with repository defaults for all optional policies.
    pub fn new(toolchain: Arc<nia_toolchain::ToolchainLayout>) -> Self {
        Self {
            toolchain,
            root: None,
            packages: Vec::new(),
            workspace_all: false,
            lock_policy: nia_package_metadata::LockPolicy::default(),
            step: None,
            timings: TimingMode::Off,
            timing_format: TimingFormat::Text,
            max_parallel_actions: None,
            optimization: OptimizationMode::O0,
            profile: BuildProfile::Debug,
            link_time_optimization: LinkTimeOptimization::Off,
            list: false,
            keep_going: false,
            dry_run: false,
            explain: false,
            print_plan: false,
            print_observations: false,
            print_cache: false,
            output_json: false,
            forward_output: true,
            progress_observer: None,
            process_output_writer: None,
        }
    }

    /// Selects the package root from which `build.nia` is discovered.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Adds a workspace member package to the invocation.
    pub fn with_package(mut self, package: impl Into<String>) -> Self {
        self.packages.push(package.into());
        self
    }

    /// Selects every workspace member.
    pub fn with_workspace_all(mut self, enabled: bool) -> Self {
        self.workspace_all = enabled;
        self
    }

    /// Selects a named build step.
    pub fn with_step(mut self, step: impl Into<String>) -> Self {
        self.step = Some(step.into());
        self
    }

    /// Enables or disables compiler timing output.
    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    /// Selects the timing output format.
    pub fn with_timing_format(mut self, timing_format: TimingFormat) -> Self {
        self.timing_format = timing_format;
        self
    }

    /// Limits the number of ready actions that may run concurrently.
    pub fn with_max_parallel_actions(mut self, max_parallel_actions: NonZeroUsize) -> Self {
        self.max_parallel_actions = Some(max_parallel_actions);
        self
    }

    /// Selects the optimization mode for compiler actions.
    pub fn with_optimization(mut self, optimization: OptimizationMode) -> Self {
        self.optimization = optimization;
        self
    }

    /// Selects the profile used for conditional source selection.
    pub fn with_profile(mut self, profile: BuildProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Selects cross-module optimization for runners and executable actions.
    pub fn with_link_time_optimization(mut self, policy: LinkTimeOptimization) -> Self {
        self.link_time_optimization = policy;
        self
    }

    /// Restricts how dependency resolution may change `nia.lock`.
    pub fn with_lock_policy(mut self, policy: nia_package_metadata::LockPolicy) -> Self {
        self.lock_policy = policy;
        self
    }

    /// Lists the plan's steps without executing actions.
    pub fn with_list(mut self, enabled: bool) -> Self {
        self.list = enabled;
        self
    }

    /// Keeps running independent actions after one fails.
    pub fn with_keep_going(mut self, enabled: bool) -> Self {
        self.keep_going = enabled;
        self
    }

    /// Freezes the build graph without executing compiler or process actions.
    pub fn with_dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// Prints the selected graph and cache explanation.
    pub fn with_explain(mut self, enabled: bool) -> Self {
        self.explain = enabled;
        self
    }

    /// Prints a deterministic summary of the canonical plan.
    pub fn with_print_plan(mut self, enabled: bool) -> Self {
        self.print_plan = enabled;
        self
    }

    /// Prints the runner's configuration observation manifest.
    pub fn with_print_observations(mut self, enabled: bool) -> Self {
        self.print_observations = enabled;
        self
    }

    /// Prints runner, configuration, and action-cache decisions.
    pub fn with_print_cache(mut self, enabled: bool) -> Self {
        self.print_cache = enabled;
        self
    }

    /// Selects JSON for plan, cache, and observation inspection output.
    pub fn with_output_json(mut self, enabled: bool) -> Self {
        self.output_json = enabled;
        self
    }

    /// Selects live child output; captured failure tails remain available either way.
    pub fn with_forward_output(mut self, enabled: bool) -> Self {
        self.forward_output = enabled;
        self
    }

    /// Observes build lifecycle and selected action events.
    pub fn with_progress_observer(mut self, observer: Arc<dyn ProgressObserver>) -> Self {
        self.progress_observer = Some(observer);
        self
    }

    /// Coordinates forwarded child output with an invocation-local renderer.
    pub fn with_process_output_writer(mut self, writer: Arc<dyn ProcessOutputWriter>) -> Self {
        self.process_output_writer = Some(writer);
        self
    }
}

impl fmt::Debug for BuildRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuildRequest")
            .field("toolchain", &self.toolchain)
            .field("root", &self.root)
            .field("packages", &self.packages)
            .field("workspace_all", &self.workspace_all)
            .field("lock_policy", &self.lock_policy)
            .field("step", &self.step)
            .field("timings", &self.timings)
            .field("timing_format", &self.timing_format)
            .field("max_parallel_actions", &self.max_parallel_actions)
            .field("optimization", &self.optimization)
            .field("profile", &self.profile)
            .field("link_time_optimization", &self.link_time_optimization)
            .field("list", &self.list)
            .field("keep_going", &self.keep_going)
            .field("dry_run", &self.dry_run)
            .field("explain", &self.explain)
            .field("print_plan", &self.print_plan)
            .field("print_observations", &self.print_observations)
            .field("print_cache", &self.print_cache)
            .field("output_json", &self.output_json)
            .field("forward_output", &self.forward_output)
            .finish_non_exhaustive()
    }
}

impl PartialEq for BuildRequest {
    fn eq(&self, other: &Self) -> bool {
        self.toolchain == other.toolchain
            && self.root == other.root
            && self.packages == other.packages
            && self.workspace_all == other.workspace_all
            && self.lock_policy == other.lock_policy
            && self.step == other.step
            && self.timings == other.timings
            && self.timing_format == other.timing_format
            && self.max_parallel_actions == other.max_parallel_actions
            && self.optimization == other.optimization
            && self.profile == other.profile
            && self.link_time_optimization == other.link_time_optimization
            && self.list == other.list
            && self.keep_going == other.keep_going
            && self.dry_run == other.dry_run
            && self.explain == other.explain
            && self.print_plan == other.print_plan
            && self.print_observations == other.print_observations
            && self.print_cache == other.print_cache
            && self.output_json == other.output_json
            && self.forward_output == other.forward_output
    }
}

impl Eq for BuildRequest {}

/// Fully resolved, invocation-local paths and build policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInvocation {
    /// Toolchain resources used by this invocation.
    pub toolchain: Arc<nia_toolchain::ToolchainLayout>,
    /// Root directory of the package being built.
    pub package_root: PathBuf,
    /// Relocation-independent local package source identity.
    pub package_source_fingerprint: [u8; 32],
    /// Package selection report shown by build inspection.
    pub package_graph_report: nia_package_metadata::PackageGraphReport,
    /// Physical path to the package's `build.nia` source.
    pub build_script: PathBuf,
    /// Executables and test suites the root manifest declares, which the
    /// runner declares before it runs `build.nia`.
    pub manifest_artifacts: Vec<nia_package_metadata::ManifestArtifact>,
    /// Invocation-local build output directory.
    pub build_dir: PathBuf,
    /// Invocation-local persistent action-cache directory.
    pub cache_dir: PathBuf,
    /// Root of the content-addressed compiler caches: the build runner and
    /// compiled objects, archives, and links. These records are keyed by
    /// content and toolchain alone, so packages may share one root; it is
    /// `cache_dir` unless the test harness names a shared one.
    pub artifact_cache_dir: PathBuf,
    /// Directory of the invocation-private runner, configuration, and draft.
    pub tmp_dir: PathBuf,
    /// Path to the generated runner executable.
    pub runner_executable: PathBuf,
    /// Private runner configuration handoff path.
    pub runner_config: PathBuf,
    /// Temporary draft-plan handoff path.
    pub plan_draft: PathBuf,
    /// Durable canonical plan path.
    pub plan_path: PathBuf,
    /// Selected build-step policy.
    pub step: BuildStepSelection,
    /// Whether independent actions keep running after one fails.
    pub keep_going: bool,
    /// Live child output policy inherited from the request.
    pub forward_output: bool,
    /// Timing policy inherited from the request.
    pub timings: TimingMode,
    /// Timing format inherited from the request.
    pub timing_format: TimingFormat,
    /// Ready-action parallelism inherited from the request.
    pub max_parallel_actions: Option<NonZeroUsize>,
    /// Optimization mode inherited from the request.
    pub optimization: OptimizationMode,
    /// Build profile inherited from the request.
    pub profile: BuildProfile,
    /// Cross-module optimization inherited from the request.
    pub link_time_optimization: LinkTimeOptimization,
}

pub(crate) fn package_source_root(
    invocation: &BuildInvocation,
    package: &PlanPackage,
) -> Option<PathBuf> {
    if package.key == PackageKey::root() {
        return Some(invocation.package_root.clone());
    }
    if let PlanPackageSource::Dependency(name) = &package.source {
        return invocation
            .package_graph_report
            .declared
            .iter()
            .find(|dependency| {
                dependency.owner_root.is_empty() && dependency.dependency.name == *name
            })
            .and_then(|dependency| dependency.source_root.clone())
            .map(normalize_host_path);
    }
    let PlanPackageSource::Local(local_root) = &package.source else {
        return None;
    };
    let mut root = invocation.package_root.clone();
    root.extend(local_root.split('/'));
    Some(root)
}

#[cfg(windows)]
fn normalize_host_path(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(path) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{path}"))
    } else if let Some(path) = text.strip_prefix(r"\\?\") {
        PathBuf::from(path)
    } else {
        path
    }
}

#[cfg(not(windows))]
fn normalize_host_path(path: PathBuf) -> PathBuf {
    path
}

/// How the coordinator selects a step from a frozen build plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStepSelection {
    /// Use the plan's declared default step.
    Default,
    /// Use the named step after semantic validation.
    Named(String),
    /// Select nothing and list the plan's steps.
    List,
}

/// Source text and logical path used to compile the generated runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRunnerSource {
    /// Stable logical source path presented to the compiler.
    pub path: String,
    /// Generated Nia source text.
    pub source: String,
}

/// Failure at one of the build bootstrap, runner, handoff, or execution stages.
#[derive(Debug)]
pub enum BuildError {
    /// Reading the process current directory failed.
    CurrentDirectory {
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Creating the invocation build directory failed.
    CreateBuildDirectory {
        /// Build directory path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Creating the invocation action-cache directory failed.
    CreateCacheDirectory {
        /// Cache directory path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Resolving local package manifests and selected dependency classes failed.
    ResolvePackageGraph {
        /// Package root where graph resolution began.
        path: PathBuf,
        /// Underlying manifest, path, protocol identity, or graph error.
        error: io::Error,
    },
    /// Refreshing `nia.lock` for `nia update` failed.
    UpdateLock {
        /// Directory from which the manifest search began.
        path: PathBuf,
        /// Underlying manifest, fetch, or lock file error.
        error: io::Error,
    },
    /// `nia clean` could not inspect, lock, or remove build state.
    Clean {
        /// Path the failed operation named.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Workspace members could not be selected for this build request.
    WorkspaceSelection {
        /// Directory from which workspace discovery began.
        path: PathBuf,
        /// Selection or manifest validation failure.
        detail: String,
    },
    /// A build plan references a dependency that is not available from the root manifest.
    InvalidDependencyReference {
        /// Plan draft containing the invalid reference.
        path: PathBuf,
        /// Reason the dependency reference was rejected.
        detail: String,
    },
    /// A path required by generated runner source is not valid UTF-8.
    NonUtf8Path {
        /// Semantic role of the path.
        role: &'static str,
        /// Rejected physical path.
        path: PathBuf,
    },
    /// Creating the transient runner directory failed.
    CreateRunnerDirectory {
        /// Runner directory path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Compiling the generated build runner failed.
    CompileRunner {
        /// Logical runner source path.
        path: String,
        /// Generated runner source retained for diagnostics.
        source: String,
        /// Underlying compiler-driver error.
        error: Box<DriverError>,
    },
    /// Manifest dependencies expose the same module name from different packages.
    InvalidBuildModuleRoot {
        /// Conflicting package module name.
        name: String,
        /// Explanation for the conflict.
        reason: String,
    },
    /// Starting, waiting for, or capturing the generated build runner failed.
    RunRunner {
        /// Runner executable path.
        path: PathBuf,
        /// Underlying process error with its failed operation or stream.
        error: io::Error,
        /// Bounded captured standard-output tail.
        stdout: Vec<u8>,
        /// Bounded captured standard-error tail.
        stderr: Vec<u8>,
    },
    /// Preparing the private plan-draft handoff failed.
    PreparePlanDraft {
        /// Draft handoff path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Publishing the private runner configuration failed.
    PrepareRunnerConfiguration {
        /// Configuration handoff path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Removing the private runner configuration failed.
    CleanupRunnerConfiguration {
        /// Configuration handoff path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// A runner-configuration field exceeds its codec limit.
    RunnerConfigurationFieldTooLarge {
        /// Field role reported to the user.
        role: &'static str,
        /// Encoded field length in bytes.
        len: usize,
    },
    /// The complete runner configuration exceeds its codec limit.
    RunnerConfigurationTooLarge {
        /// Encoded payload length in bytes.
        len: usize,
    },
    /// Reading and validating the runner-produced plan draft failed.
    ReadPlanDraft {
        /// Draft handoff path.
        path: PathBuf,
        /// Underlying bounded handoff error.
        error: PlanHandoffError,
    },
    /// Runner configuration evidence was malformed or did not match the
    /// package and environment values observed during configuration.
    RunnerObservations {
        /// Build-plan draft carrying the evidence.
        path: PathBuf,
        /// Evidence validation failure.
        error: io::Error,
    },
    /// Durably publishing the canonical frozen plan failed.
    PublishBuildPlan {
        /// Canonical plan path.
        path: PathBuf,
        /// Underlying bounded handoff error.
        error: PlanHandoffError,
    },
    /// Executing the canonical plan failed.
    ExecuteBuildPlan {
        /// Underlying coordinator failure.
        error: Box<CoordinatorError>,
    },
    /// Removing the transient plan draft failed.
    CleanupPlanDraft {
        /// Draft handoff path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Removing the transient runner executable failed.
    CleanupRunnerExecutable {
        /// Runner executable path.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// The generated build runner exited unsuccessfully.
    RunnerFailed {
        /// Runner executable path.
        path: PathBuf,
        /// Child exit status.
        status: ExitStatus,
        /// Bounded captured standard output.
        stdout: Vec<u8>,
        /// Bounded captured standard error.
        stderr: Vec<u8>,
    },
    /// No `build.nia` exists at or above the requested start directory.
    MissingBuildScript {
        /// Directory where upward discovery started.
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
            Self::ResolvePackageGraph { path, error } => write!(
                f,
                "failed to resolve package graph at `{}`: {error}",
                path.display()
            ),
            Self::UpdateLock { path, error } => write!(
                f,
                "failed to update nia.lock from `{}`: {error}",
                path.display()
            ),
            Self::Clean { path, error } => {
                write!(f, "failed to clean `{}`: {error}", path.display())
            }
            Self::WorkspaceSelection { path, detail } => write!(
                f,
                "failed to select workspace packages from `{}`: {detail}",
                path.display()
            ),
            Self::InvalidDependencyReference { path, detail } => write!(
                f,
                "build plan `{}` has an invalid manifest dependency reference: {detail}",
                path.display()
            ),
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
            Self::InvalidBuildModuleRoot { name, reason } => {
                write!(f, "manifest package root `{name}` is ambiguous: {reason}")
            }
            Self::RunRunner {
                path,
                error,
                stdout,
                stderr,
            } => {
                write!(
                    f,
                    "failed to run build runner `{}`: {error}",
                    path.display()
                )?;
                write_runner_output(f, "stdout", stdout)?;
                write_runner_output(f, "stderr", stderr)
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
            Self::RunnerObservations { path, error } => write!(
                f,
                "failed to validate runner observations in `{}`: {error}",
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
                    "build runner `{}` {}",
                    path.display(),
                    nia_diagnostic::describe_exit_status(*status)
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

/// Removes the outputs, caches, and transient files of the package at or
/// above `root` (the current directory when absent) for `nia clean`.
///
/// Discovery must find a `nia.toml` or `build.nia`, so `nia clean` never
/// touches a `build/` that no Nia package owns. Fetched dependencies and the
/// registry cache are kept. The package-state lock is held throughout, so a
/// concurrent build or update waits instead of racing the removal, and
/// `build/` itself must be a real directory, so the removal cannot follow a
/// link out of the package.
pub fn run_clean(root: Option<PathBuf>) -> Result<(), BuildError> {
    let start = match root {
        Some(root) if root.is_absolute() => root,
        Some(root) => env::current_dir()
            .map_err(|error| BuildError::CurrentDirectory { error })?
            .join(root),
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let layout = match nia_package_metadata::BuildLayout::discover(&start) {
        Ok(layout) => layout,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(BuildError::MissingBuildScript { start });
        }
        Err(error) => return Err(BuildError::Clean { path: start, error }),
    };
    let build_root = layout.root();
    let clean = |error, path: &Path| BuildError::Clean {
        path: path.to_path_buf(),
        error,
    };
    match fs::symlink_metadata(&build_root) {
        Ok(metadata) if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_dir() => {
            return Err(clean(
                io::Error::new(io::ErrorKind::InvalidData, "not a real directory"),
                &build_root,
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(clean(error, &build_root)),
    }
    let _lock = layout
        .lock_state()
        .map_err(|error| clean(error, &layout.state_lock()))?;
    for path in [layout.out_dir(), layout.cache_dir(), layout.tmp_dir()] {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(clean(error, &path)),
        };
        // `remove_dir_all` does not follow links inside the tree; a link in
        // place of the directory itself is removed as the link.
        let result = if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            fs::remove_file(&path)
        } else {
            fs::remove_dir_all(&path)
        };
        result.map_err(|error| clean(error, &path))?;
    }
    Ok(())
}

/// Refreshes Git selectors and registry versions in `nia.lock` for
/// `nia update`. `root` defaults to the current directory; an empty `names`
/// updates every locked dependency.
pub fn run_update(
    root: Option<PathBuf>,
    names: &[String],
) -> Result<Vec<nia_package_metadata::LockUpdate>, BuildError> {
    let start = match root {
        Some(root) if root.is_absolute() => root,
        Some(root) => env::current_dir()
            .map_err(|error| BuildError::CurrentDirectory { error })?
            .join(root),
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    nia_package_metadata::update_lock(&start, names)
        .map_err(|error| BuildError::UpdateLock { path: start, error })
}

/// Runs one build request from package discovery through action publication.
pub fn run_build(request: BuildRequest) -> Result<(), BuildError> {
    let Some(selected) = select_workspace_members(&request)? else {
        return run_build_single(request);
    };
    if selected.len() > 1 {
        nia_package_metadata::PackageGraph::resolve_members_with_policy(
            &selected,
            request.lock_policy,
        )
        .map_err(|error| BuildError::ResolvePackageGraph {
            path: selected[0].root.clone(),
            error,
        })?;
    }
    let mut first_error = None;
    for member in selected {
        let mut member_request = request.clone();
        member_request.root = Some(member.root);
        member_request.packages.clear();
        member_request.workspace_all = false;
        if let Err(error) = run_build_single(member_request) {
            if !request.keep_going {
                return Err(error);
            }
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn select_workspace_members(
    request: &BuildRequest,
) -> Result<Option<Vec<nia_package_metadata::WorkspaceMember>>, BuildError> {
    if request.workspace_all && !request.packages.is_empty() {
        return Err(BuildError::WorkspaceSelection {
            path: request.root.clone().unwrap_or_default(),
            detail: "`--workspace` cannot be combined with `--package`".to_owned(),
        });
    }
    let start = match &request.root {
        Some(root) if root.is_absolute() => root.clone(),
        Some(root) => env::current_dir()
            .map_err(|error| BuildError::CurrentDirectory { error })?
            .join(root),
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let workspace = nia_package_metadata::WorkspaceManifest::discover(&start).map_err(|error| {
        BuildError::WorkspaceSelection {
            path: start.clone(),
            detail: error.to_string(),
        }
    })?;
    let Some(workspace) = workspace else {
        if request.workspace_all || !request.packages.is_empty() {
            return Err(BuildError::WorkspaceSelection {
                path: start,
                detail: "`--workspace` and `--package` require a workspace manifest".to_owned(),
            });
        }
        return Ok(None);
    };
    let members = workspace
        .member_packages()
        .map_err(|error| BuildError::WorkspaceSelection {
            path: workspace.root.clone(),
            detail: error.to_string(),
        })?;
    let selected_names = if !request.packages.is_empty() {
        let mut selected = Vec::with_capacity(request.packages.len());
        for name in &request.packages {
            let Some(member) = members.iter().find(|member| member.name == *name) else {
                return Err(BuildError::WorkspaceSelection {
                    path: workspace.root.clone(),
                    detail: format!("package `{name}` is not a workspace member"),
                });
            };
            if !selected
                .iter()
                .any(|selected: &String| selected == &member.name)
            {
                selected.push(member.name.clone());
            }
        }
        selected
    } else if request.workspace_all {
        members.iter().map(|member| member.name.clone()).collect()
    } else if start
        .canonicalize()
        .is_ok_and(|start| start == workspace.root)
    {
        let roots = workspace
            .default_members
            .as_ref()
            .unwrap_or(&workspace.members)
            .iter()
            .map(|path| {
                if path == "." {
                    workspace.root.clone()
                } else {
                    workspace.root.join(path)
                }
            })
            .collect::<Vec<_>>();
        members
            .iter()
            .filter(|member| roots.contains(&member.root))
            .map(|member| member.name.clone())
            .collect()
    } else {
        let start = start.canonicalize().unwrap_or(start);
        members
            .iter()
            .filter(|member| start.starts_with(&member.root))
            .max_by_key(|member| member.root.components().count())
            .map(|member| vec![member.name.clone()])
            .unwrap_or_default()
    };
    if selected_names.is_empty() {
        return Err(BuildError::WorkspaceSelection {
            path: workspace.root.clone(),
            detail: "the selected workspace configuration contains no packages".to_owned(),
        });
    }
    let selected = selected_names
        .iter()
        .filter_map(|name| members.iter().find(|member| member.name == *name).cloned())
        .collect();
    Ok(Some(selected))
}

fn run_build_single(request: BuildRequest) -> Result<(), BuildError> {
    let dry_run = request.dry_run;
    let explain = request.explain;
    let print_plan = request.print_plan;
    let print_observations = request.print_observations;
    let print_cache = request.print_cache;
    let output_json = request.output_json;
    let process_output_writer = request.process_output_writer.clone();
    let mut progress_guard =
        ProgressBuildGuard::new(request.progress_observer.clone().map(ProgressSession::new));
    let result = nia_timing::collect_to_stderr(
        TimingOptions::new(request.timings).with_format(request.timing_format),
        || {
            let result = (|| {
                let timings = request.timings;
                let invocation = time_summary_stage(timings, "build_resolve_invocation", || {
                    progress_phase(progress_guard.session(), ProgressPhase::Resolve, || {
                        resolve_build_invocation(request)
                    })
                })?;
                time_summary_stage(timings, "build_prepare_directories", || {
                    progress_phase(progress_guard.session(), ProgressPhase::Prepare, || {
                        prepare_build_directories(&invocation)
                    })
                })?;
                nia_timing::emit_counter("build.runner_compilations", 1);
                let runner = time_summary_stage(timings, "build_compile_runner", || {
                    progress_phase(progress_guard.session(), ProgressPhase::Runner, || {
                        compile_build_runner(&invocation, progress_guard.session())
                    })
                });
                if runner.is_err() {
                    nia_timing::emit_counter("build.runner_compile_failures", 1);
                }
                let (runner_executable, runner_cache_key) = runner?;
                let configured = time_summary_stage(timings, "build_configure_plan", || {
                    progress_phase(progress_guard.session(), ProgressPhase::Configure, || {
                        let mut configure_candidate = match configure_cache_candidate(
                            &invocation,
                            &runner_cache_key,
                            progress_guard.session(),
                        ) {
                            Ok(ConfigureCacheCandidate::Present) => {
                                nia_timing::emit_counter("build.configure_cache_candidates", 1);
                                ConfigureCacheCandidate::Present
                            }
                            Ok(ConfigureCacheCandidate::None) => ConfigureCacheCandidate::None,
                            Ok(candidate @ ConfigureCacheCandidate::Miss(_)) => candidate,
                            Err(_) => ConfigureCacheCandidate::None,
                        };
                        let reusable = ConfigureCache::new(&invocation)
                            .with_lock_observer(progress_guard.session().map(|progress| {
                                progress.phase_lock_observer(ProgressPhase::Configure)
                            }))
                            .lookup_reusable(
                                &runner_cache_key,
                                &invocation_identity(&invocation),
                                &invocation.package_root,
                            )
                            .inspect(|lookup| {
                                if let ConfigureCacheLookup::Miss(reason) = lookup
                                    && !matches!(reason, ConfigureCacheMissReason::NotFound)
                                {
                                    configure_candidate =
                                        ConfigureCacheCandidate::Miss(reason.clone());
                                }
                            })
                            .ok();
                        let mut runner_reused = false;
                        let draft = match reusable {
                            Some(ConfigureCacheLookup::Hit(record)) => {
                                let manifest = record.manifest.encode().ok();
                                match (ConfigureCache::decode_plan(&record).ok(), manifest) {
                                    (Some(plan), Some(observations)) => {
                                        runner_reused = true;
                                        nia_timing::emit_counter("build.configure_cache_hits", 1);
                                        let cleanup = fs::remove_file(&runner_executable);
                                        match cleanup {
                                            Ok(()) => Ok(BuildDraft { plan, observations }),
                                            Err(error)
                                                if error.kind() == io::ErrorKind::NotFound =>
                                            {
                                                Ok(BuildDraft { plan, observations })
                                            }
                                            Err(error) => {
                                                Err(BuildError::CleanupRunnerExecutable {
                                                    path: runner_executable.clone(),
                                                    error,
                                                })
                                            }
                                        }
                                    }
                                    _ => {
                                        nia_timing::emit_counter("build.configure_cache_misses", 1);
                                        nia_timing::emit_counter("build.runner_executions", 1);
                                        run_and_cleanup_build_runner_draft(
                                            &invocation,
                                            &runner_executable,
                                            &runner_cache_key,
                                            process_output_writer.clone(),
                                            progress_guard.session(),
                                        )
                                    }
                                }
                            }
                            _ => {
                                nia_timing::emit_counter("build.configure_cache_misses", 1);
                                nia_timing::emit_counter("build.runner_executions", 1);
                                run_and_cleanup_build_runner_draft(
                                    &invocation,
                                    &runner_executable,
                                    &runner_cache_key,
                                    process_output_writer.clone(),
                                    progress_guard.session(),
                                )
                            }
                        };
                        if let Some(progress) = progress_guard.session() {
                            progress.emit(ProgressEventKind::PhaseCacheDecision {
                                phase: ProgressPhase::Configure,
                                reused: runner_reused,
                            });
                        }
                        draft.map(|draft| (draft, configure_candidate, runner_reused))
                    })
                });
                if configured.is_err() {
                    nia_timing::emit_counter("build.runner_failures", 1);
                }
                let (draft, configure_candidate, runner_reused) = configured?;
                validate_manifest_dependency_references(draft.plan.packages(), &invocation)
                    .map_err(|detail| BuildError::InvalidDependencyReference {
                        path: invocation.plan_draft.clone(),
                        detail,
                    })?;
                let plan = draft.plan;
                let preview =
                    progress_phase(progress_guard.session(), ProgressPhase::Preview, || {
                        preview_build_plan(&plan, &invocation)
                    })
                    .map_err(|error| BuildError::ExecuteBuildPlan {
                        error: Box::new(error),
                    })?;
                progress_guard.select(preview.actions.clone());
                if let Some(progress) = progress_guard.session() {
                    progress.emit(ProgressEventKind::SelectionReady {
                        total: preview.actions.len(),
                        actions: preview.actions.clone(),
                    });
                    for key in &preview.actions {
                        if let Ok(index) = plan
                            .actions()
                            .binary_search_by(|action| action.key.cmp(key))
                        {
                            let action = &plan.actions()[index];
                            progress.emit(ProgressEventKind::ActionDescribed {
                                action: key.clone(),
                                description: progress_action_description(&plan, action),
                            });
                        }
                    }
                }
                if print_plan {
                    if output_json {
                        print!("{}", render_build_plan_json(&plan));
                    } else {
                        print!("{}", render_build_plan_summary(&plan));
                    }
                }
                if explain {
                    if output_json {
                        print!(
                            "{}",
                            render_build_explanation_json_with_observations(
                                &plan,
                                &invocation,
                                configure_candidate.clone(),
                                &preview,
                                &draft.observations,
                            )
                        );
                    } else {
                        print!(
                            "{}",
                            render_build_explanation_with_observations(
                                &plan,
                                &invocation,
                                configure_candidate.clone(),
                                &preview,
                                &draft.observations,
                            )
                        );
                    }
                }
                if print_observations {
                    if output_json {
                        print!(
                            "{}",
                            render_configure_observations_json(&draft.observations)
                        );
                    } else {
                        print!("{}", render_configure_observations(&draft.observations));
                    }
                }
                if print_cache || explain {
                    if output_json {
                        print!(
                            "{}",
                            render_configure_cache_json(configure_candidate, runner_reused)
                        );
                    } else {
                        println!(
                            "configure cache: {}; runner {}",
                            configure_candidate.clone(),
                            if runner_reused { "reused" } else { "executed" }
                        );
                    }
                }
                if invocation.step == BuildStepSelection::List {
                    if output_json {
                        print!("{}", render_step_list_json(&plan));
                    } else {
                        print!("{}", render_step_list(&plan));
                    }
                    return Ok(());
                }
                if dry_run {
                    if print_cache || explain {
                        if output_json {
                            print!("{}", render_action_cache_preview_json(&plan, &preview));
                        } else {
                            print!("{}", render_action_cache_preview_summary(&plan, &preview));
                        }
                    }
                    return Ok(());
                }
                if let Some(limit) = invocation.max_parallel_actions {
                    nia_timing::emit_counter("build.action_parallelism_limit", limit.get() as u64);
                }
                let result = time_summary_stage(timings, "build_execute_plan", || {
                    progress_phase(progress_guard.session(), ProgressPhase::Execute, || {
                        execute_build_plan_with_progress(
                            &plan,
                            &invocation,
                            progress_guard.session(),
                            process_output_writer.clone(),
                        )
                        .map_err(|error| BuildError::ExecuteBuildPlan {
                            error: Box::new(error),
                        })
                    })
                });
                if let Ok(report) = &result {
                    nia_timing::emit_counter("build.steps_executed", report.steps.len() as u64);
                    nia_timing::emit_counter("build.actions_executed", report.actions.len() as u64);
                    emit_action_cache_counters(report);
                    if print_cache || explain {
                        if output_json {
                            print!("{}", render_action_cache_json(report, &plan));
                        } else {
                            print!("{}", render_action_cache_summary(report));
                        }
                    }
                } else {
                    nia_timing::emit_counter("build.action_failures", 1);
                }
                result.map(|_| ())
            })();
            progress_guard.finish(result.is_ok());
            result
        },
    );
    progress_guard.finish(result.is_ok());
    result
}

fn configure_cache_candidate(
    invocation: &BuildInvocation,
    runner_cache_key: &str,
    progress: Option<&ProgressSession>,
) -> io::Result<ConfigureCacheCandidate> {
    ConfigureCache::new(invocation)
        .with_lock_observer(
            progress.map(|progress| progress.phase_lock_observer(ProgressPhase::Configure)),
        )
        .candidate_status(runner_cache_key, &invocation_identity(invocation))
}

fn progress_phase<T, E>(
    progress: Option<&ProgressSession>,
    phase: ProgressPhase,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    match progress {
        Some(progress) => progress.phase(phase, operation),
        None => operation(),
    }
}

/// One `name kind` line per step, in canonical order, marking the default.
fn render_step_list(plan: &BuildPlan) -> String {
    use std::fmt::Write as _;
    let width = plan
        .steps()
        .iter()
        .map(|step| step.key.name().len())
        .max()
        .unwrap_or(0);
    let mut output = String::new();
    for step in plan.steps() {
        let _ = write!(
            output,
            "{:width$}  {}",
            step.key.name(),
            step_kind_name(plan, step)
        );
        if plan.default_step() == Some(&step.key) {
            output.push_str(" (default)");
        }
        output.push('\n');
    }
    output
}

fn render_step_list_json(plan: &BuildPlan) -> String {
    let steps = plan
        .steps()
        .iter()
        .map(|step| {
            serde_json::json!({
                "name": step.key.name(),
                "kind": step_kind_name(plan, step),
                "default": plan.default_step() == Some(&step.key),
            })
        })
        .collect::<Vec<_>>();
    format!(
        "{}\n",
        serde_json::json!({ "kind": "steps", "steps": steps })
    )
}

fn step_kind_name(plan: &BuildPlan, step: &PlanStep) -> &'static str {
    plan.actions()
        .binary_search_by(|action| action.key.cmp(&step.action))
        .map_or("unknown", |index| action_kind_name(&plan.actions()[index]))
}

/// Renders a deterministic human-readable summary of a frozen build plan.
pub fn render_build_plan_summary(plan: &BuildPlan) -> String {
    let mut output = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(output, "build plan:");
    let _ = writeln!(output, "  root package: {}", plan.root_package().as_str());
    let _ = writeln!(output, "  host target: {}", plan.host_target().name());
    let _ = writeln!(
        output,
        "  artifact target: {}",
        plan.artifact_target().name()
    );
    let _ = writeln!(output, "  packages: {}", plan.packages().len());
    let _ = writeln!(output, "  modules: {}", plan.modules().len());
    let _ = writeln!(output, "  artifacts: {}", plan.artifacts().len());
    let _ = writeln!(output, "  actions: {}", plan.actions().len());
    let _ = writeln!(output, "  steps: {}", plan.steps().len());
    for module in plan.modules() {
        let _ = writeln!(
            output,
            "  module {}::{}: source={}, optimization={}",
            module.key.package().as_str(),
            module.key.name(),
            module.root_source.protocol_path(),
            optimization_name(module.optimization),
        );
    }
    for artifact in plan.artifacts() {
        let _ = writeln!(
            output,
            "  artifact {}::{}: kind={}, output={}",
            artifact.key.package().as_str(),
            artifact.key.name(),
            artifact_kind_name(&artifact.kind),
            artifact.output.protocol_path(),
        );
    }
    for action in plan.actions() {
        let _ = writeln!(
            output,
            "  action {}::{}: kind={}, outputs={}",
            action.key.package().as_str(),
            action.key.name(),
            action_kind_name(action),
            action_outputs_json(plan, action).len(),
        );
    }
    for step in plan.steps() {
        let _ = writeln!(
            output,
            "  step {}::{}: action={}, dependencies={}",
            step.key.package().as_str(),
            step.key.name(),
            format_args!("{}::{}", step.action.package().as_str(), step.action.name()),
            step.dependencies.len(),
        );
    }
    for usage in plan.package_usage() {
        let _ = writeln!(
            output,
            "  package {}: modules={}, artifacts={}, actions={}, steps={}, loaded={}, used={}",
            usage.package.as_str(),
            usage.loaded_modules,
            usage.artifacts,
            usage.actions,
            usage.steps,
            usage.is_loaded(),
            usage.is_used(),
        );
    }
    output
}

/// Renders the configuration inputs that explain the selected plan.
pub fn render_build_explanation(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    configure_candidate: ConfigureCacheCandidate,
    preview: &ExecutionReport,
) -> String {
    render_build_explanation_with_observations(plan, invocation, configure_candidate, preview, &[])
}

/// Renders a deterministic explanation with the runner observation manifest.
pub fn render_build_explanation_with_observations(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    configure_candidate: ConfigureCacheCandidate,
    preview: &ExecutionReport,
    observations: &[u8],
) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    let _ = writeln!(output, "build explanation:");
    let _ = writeln!(
        output,
        "  package root: {}",
        invocation.package_root.display()
    );
    let _ = writeln!(
        output,
        "  package graph fingerprint: {}",
        hex_fingerprint(&invocation.package_source_fingerprint)
    );
    let _ = writeln!(
        output,
        "  configuration: profile={}, optimization={}, lto={}",
        profile_name(invocation.profile),
        optimization_name(invocation.optimization),
        link_time_optimization_name(invocation.link_time_optimization),
    );
    let graph = &invocation.package_graph_report;
    let _ = writeln!(
        output,
        "  package graph: declared_dependencies={}, resolved_dependencies={}, selected_packages={}",
        graph.declared.len(),
        graph
            .declared
            .iter()
            .filter(|dependency| dependency.resolved_root.is_some())
            .count(),
        graph.resolved.len()
    );
    for usage in build_explanation_dependency_usage(plan, invocation, preview) {
        let dependency = usage.dependency;
        let owner = if dependency.owner_root.is_empty() {
            plan.root_package().as_str()
        } else {
            dependency.owner_root.as_str()
        };
        let _ = writeln!(
            output,
            "    {}::{} ({:?}) -> {} [resolved={}, loaded_modules={}, used={}]",
            owner,
            dependency.dependency.name,
            dependency.dependency.scope,
            dependency.resolved_root.as_deref().unwrap_or("filtered"),
            usage.resolved,
            usage.loaded_modules,
            usage.used,
        );
    }
    let _ = writeln!(
        output,
        "  configure cache candidate: {}",
        configure_candidate
    );
    let _ = writeln!(
        output,
        "  selected step: {}",
        plan.selected_step()
            .or_else(|| plan.default_step())
            .map_or("<none>", StepKey::name)
    );
    let _ = writeln!(
        output,
        "  target: host={}, artifact={}",
        plan.host_target().name(),
        plan.artifact_target().name()
    );
    let _ = writeln!(
        output,
        "  action closure: {} actions across {} steps",
        preview.actions.len(),
        preview.steps.len()
    );
    for action in &preview.actions {
        let _ = writeln!(
            output,
            "    {}::{}",
            action.package().as_str(),
            action.name()
        );
    }
    let observation_count = ConfigureManifest::decode(observations)
        .map(|manifest| manifest.observations().len())
        .unwrap_or(0);
    let _ = writeln!(output, "  observations: {observation_count}");
    let tools = plan_tool_declarations(plan);
    let _ = writeln!(output, "  tools: {}", tools.len());
    for tool in tools {
        let _ = writeln!(output, "    {}", tool);
    }
    output
}

struct BuildExplanationDependencyUsage<'a> {
    dependency: &'a nia_package_metadata::PackageDependencyReport,
    resolved: bool,
    loaded_modules: usize,
    used: bool,
}

fn build_explanation_dependency_usage<'a>(
    plan: &BuildPlan,
    invocation: &'a BuildInvocation,
    preview: &ExecutionReport,
) -> Vec<BuildExplanationDependencyUsage<'a>> {
    let mut packages_by_source_root: BTreeMap<PathBuf, Vec<&PlanPackage>> = BTreeMap::new();
    for package in plan.packages() {
        if let Some(source_root) =
            package_source_root(invocation, package).and_then(|path| path.canonicalize().ok())
        {
            packages_by_source_root
                .entry(source_root)
                .or_default()
                .push(package);
        }
    }

    let loaded_modules = plan
        .package_usage()
        .into_iter()
        .map(|usage| (usage.package, usage.loaded_modules))
        .collect::<BTreeMap<_, _>>();
    let used_packages = build_explanation_closure_packages(plan, invocation, preview);

    invocation
        .package_graph_report
        .declared
        .iter()
        .map(|dependency| {
            let packages = dependency
                .source_root
                .as_ref()
                .and_then(|root| packages_by_source_root.get(root));
            BuildExplanationDependencyUsage {
                dependency,
                resolved: dependency.resolved_root.is_some(),
                loaded_modules: packages.map_or(0, |packages| {
                    packages
                        .iter()
                        .map(|package| loaded_modules.get(&package.key).copied().unwrap_or(0))
                        .sum()
                }),
                used: packages.is_some_and(|packages| {
                    packages
                        .iter()
                        .any(|package| used_packages.contains(&package.key))
                }),
            }
        })
        .collect()
}

fn build_explanation_closure_packages(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    preview: &ExecutionReport,
) -> BTreeSet<PackageKey> {
    let package_dependency_classes = coordinator::package_dependency_classes(plan, invocation);
    let actions = plan
        .actions()
        .iter()
        .map(|action| (action.key.clone(), action))
        .collect::<BTreeMap<_, _>>();
    let artifacts = plan
        .artifacts()
        .iter()
        .map(|artifact| (artifact.key.clone(), artifact))
        .collect::<BTreeMap<_, _>>();
    let modules = plan
        .modules()
        .iter()
        .map(|module| (module.key.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let mut modules_by_source = BTreeMap::<LogicalPath, Vec<ModuleKey>>::new();
    for module in plan.modules() {
        modules_by_source
            .entry(module.root_source.clone())
            .or_default()
            .push(module.key.clone());
    }

    let mut packages = BTreeSet::new();
    let mut visited_modules = BTreeSet::new();
    for step in &preview.steps {
        packages.insert(step.package().clone());
    }
    for action_key in &preview.actions {
        packages.insert(action_key.package().clone());
        let Some(action) = actions.get(action_key) else {
            continue;
        };
        match &action.kind {
            ActionKind::CompilerCheck { module, mode, .. } => include_build_explanation_modules(
                module,
                *mode,
                &modules,
                &modules_by_source,
                &package_dependency_classes,
                &mut visited_modules,
                &mut packages,
            ),
            ActionKind::CompilerEmit {
                artifact,
                static_archives,
                ..
            } => {
                include_build_explanation_artifact(
                    artifact,
                    &artifacts,
                    &modules,
                    &modules_by_source,
                    &package_dependency_classes,
                    &mut visited_modules,
                    &mut packages,
                );
                for archive in static_archives {
                    include_build_explanation_artifact(
                        archive,
                        &artifacts,
                        &modules,
                        &modules_by_source,
                        &package_dependency_classes,
                        &mut visited_modules,
                        &mut packages,
                    );
                }
            }
            ActionKind::ExternalCommand(command) | ActionKind::TestExecutable(command) => {
                if let CommandProgram::Path(path) = &command.program {
                    include_build_explanation_path(path, &mut packages);
                }
                include_build_explanation_path(&command.working_directory, &mut packages);
                for path in command.inputs.iter().chain(&command.outputs) {
                    include_build_explanation_path(path, &mut packages);
                }
                for argument in &command.arguments {
                    match argument {
                        CommandArgument::InputPath(path) | CommandArgument::OutputPath(path) => {
                            include_build_explanation_path(path, &mut packages);
                        }
                        CommandArgument::Literal(_) => {}
                    }
                }
            }
            ActionKind::GeneratedFile { output, .. } => {
                include_build_explanation_path(output, &mut packages);
            }
            ActionKind::InstallArtifact { artifact, .. } => {
                include_build_explanation_artifact(
                    artifact,
                    &artifacts,
                    &modules,
                    &modules_by_source,
                    &package_dependency_classes,
                    &mut visited_modules,
                    &mut packages,
                );
            }
            ActionKind::Aggregate | ActionKind::Uncacheable { .. } => {}
        }
    }
    packages
}

fn include_build_explanation_artifact(
    artifact_key: &ArtifactKey,
    artifacts: &BTreeMap<ArtifactKey, &PlanArtifact>,
    modules: &BTreeMap<ModuleKey, &PlanModule>,
    modules_by_source: &BTreeMap<LogicalPath, Vec<ModuleKey>>,
    package_dependency_classes: &BTreeMap<PackageKey, BTreeSet<DependencyScope>>,
    visited_modules: &mut BTreeSet<ModuleKey>,
    packages: &mut BTreeSet<PackageKey>,
) {
    packages.insert(artifact_key.package().clone());
    if let Some(artifact) = artifacts.get(artifact_key) {
        include_build_explanation_modules(
            &artifact.root_module,
            artifact.mode,
            modules,
            modules_by_source,
            package_dependency_classes,
            visited_modules,
            packages,
        );
    }
}

fn include_build_explanation_modules(
    module_key: &ModuleKey,
    mode: CompilationMode,
    modules: &BTreeMap<ModuleKey, &PlanModule>,
    modules_by_source: &BTreeMap<LogicalPath, Vec<ModuleKey>>,
    package_dependency_classes: &BTreeMap<PackageKey, BTreeSet<DependencyScope>>,
    visited_modules: &mut BTreeSet<ModuleKey>,
    packages: &mut BTreeSet<PackageKey>,
) {
    let mut pending = vec![module_key.clone()];
    while let Some(module_key) = pending.pop() {
        if !visited_modules.insert(module_key.clone()) {
            continue;
        }
        let Some(module) = modules.get(&module_key) else {
            continue;
        };
        packages.insert(module.key.package().clone());
        for import in &module.imports {
            let selected = match import.path.root() {
                LogicalPathRoot::Package(package) => coordinator::package_dependency_selected(
                    package_dependency_classes,
                    package,
                    mode,
                ),
                _ => true,
            };
            if !selected {
                continue;
            }
            include_build_explanation_path(&import.path, packages);
            if let Some(imported_modules) = modules_by_source.get(&import.path) {
                pending.extend(imported_modules.iter().cloned());
            }
        }
    }
}

fn include_build_explanation_path(path: &LogicalPath, packages: &mut BTreeSet<PackageKey>) {
    match path.root() {
        LogicalPathRoot::Package(package) => {
            packages.insert(package.clone());
        }
        LogicalPathRoot::Artifact(artifact) => {
            packages.insert(artifact.package().clone());
        }
        LogicalPathRoot::Build | LogicalPathRoot::Cache | LogicalPathRoot::Toolchain => {}
    }
}

fn hex_fingerprint(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn render_configure_observations(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::from("configure observations:\n");
    if bytes.is_empty() {
        output.push_str("  untracked configuration; reuse disabled\n");
    } else if let Ok(manifest) = ConfigureManifest::decode(bytes) {
        let _ = writeln!(output, "  policy: {:?}", manifest.policy());
        for observation in manifest.observations() {
            let _ = writeln!(
                output,
                "  {} {} {}",
                observation.kind(),
                observation.path().display(),
                hex_fingerprint(observation.fingerprint())
            );
        }
    } else {
        output.push_str("  invalid observation manifest; reuse disabled\n");
    }
    output
}

fn render_action_cache_summary(report: &ExecutionReport) -> String {
    use std::fmt::Write as _;
    let mut output = String::from("action cache:\n");
    for entry in &report.action_cache {
        let _ = writeln!(
            output,
            "  {}::{}: {:?}",
            entry.action.package().as_str(),
            entry.action.name(),
            entry.outcome
        );
    }
    output
}

/// Renders a stable machine-readable plan summary.
pub fn render_build_plan_json(plan: &BuildPlan) -> String {
    build_plan_json_value(plan).to_string() + "\n"
}

fn build_plan_json_value(plan: &BuildPlan) -> serde_json::Value {
    let packages = plan
        .package_usage()
        .into_iter()
        .map(|usage| {
            let package = plan
                .packages()
                .iter()
                .find(|package| package.key == usage.package)
                .expect("package usage must have a declaration");
            serde_json::json!({
                "package": usage.package.as_str(),
                "source": match &package.source {
                    PlanPackageSource::Local(root) => serde_json::json!({"local": root}),
                    PlanPackageSource::Dependency(name) => serde_json::json!({"dependency": name}),
                },
                "loaded_modules": usage.loaded_modules,
                "artifacts": usage.artifacts,
                "actions": usage.actions,
                "steps": usage.steps,
                "loaded": usage.is_loaded(),
                "used": usage.is_used(),
            })
        })
        .collect::<Vec<_>>();
    let modules = plan
        .modules()
        .iter()
        .map(|module| {
            serde_json::json!({
                "key": format!("{}::{}", module.key.package().as_str(), module.key.name()),
                "package": module.key.package().as_str(),
                "name": module.key.name(),
                "root_source": logical_path_json(&module.root_source),
                "optimization": optimization_name(module.optimization),
                "imports": module.imports.iter().map(|import| serde_json::json!({
                    "name": import.name,
                    "path": logical_path_json(&import.path),
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let artifacts = plan
        .artifacts()
        .iter()
        .map(|artifact| {
            serde_json::json!({
                "key": format!("{}::{}", artifact.key.package().as_str(), artifact.key.name()),
                "package": artifact.key.package().as_str(),
                "name": artifact.key.name(),
                "root_module": format!("{}::{}", artifact.root_module.package().as_str(), artifact.root_module.name()),
                "kind": artifact_kind_name(&artifact.kind),
                "output": logical_path_json(&artifact.output),
                "runtime": runtime_name(artifact.runtime),
                "mode": compilation_mode_name(artifact.mode),
            })
        })
        .collect::<Vec<_>>();
    let actions = plan
        .actions()
        .iter()
        .map(|action| action_json_value(plan, action))
        .collect::<Vec<_>>();
    let steps = plan
        .steps()
        .iter()
        .map(|step| {
            serde_json::json!({
                "key": format!("{}::{}", step.key.package().as_str(), step.key.name()),
                "package": step.key.package().as_str(),
                "name": step.key.name(),
                "action": format!("{}::{}", step.action.package().as_str(), step.action.name()),
                "dependencies": step.dependencies.iter().map(|dependency| format!("{}::{}", dependency.package().as_str(), dependency.name())).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "build-plan",
        "protocol_epoch": plan.protocol_epoch(),
        "root_package": plan.root_package().as_str(),
        "selection": {
            "selected_step": plan.selected_step().or_else(|| plan.default_step()).map(|step| format!("{}::{}", step.package().as_str(), step.name())),
            "explicit_step": plan.selected_step().map(|step| format!("{}::{}", step.package().as_str(), step.name())),
            "default_step": plan.default_step().map(|step| format!("{}::{}", step.package().as_str(), step.name())),
        },
        "targets": {
            "host": plan.host_target().name(),
            "artifact": plan.artifact_target().name(),
        },
        "counts": {
            "packages": plan.packages().len(),
            "modules": plan.modules().len(),
            "artifacts": plan.artifacts().len(),
            "actions": plan.actions().len(),
            "steps": plan.steps().len(),
        },
        "packages": packages,
        "modules": modules,
        "artifacts": artifacts,
        "actions": actions,
        "steps": steps,
    })
}

/// Renders a stable machine-readable build explanation.
pub fn render_build_explanation_json(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    configure_candidate: ConfigureCacheCandidate,
    preview: &ExecutionReport,
) -> String {
    render_build_explanation_json_with_observations(
        plan,
        invocation,
        configure_candidate,
        preview,
        &[],
    )
}

/// Renders a stable explanation JSON record with runner observations.
pub fn render_build_explanation_json_with_observations(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
    configure_candidate: ConfigureCacheCandidate,
    preview: &ExecutionReport,
    observations: &[u8],
) -> String {
    let actions = preview
        .actions
        .iter()
        .map(|action| format!("{}::{}", action.package().as_str(), action.name()))
        .collect::<Vec<_>>();
    let dependencies = build_explanation_dependency_usage(plan, invocation, preview)
        .into_iter()
        .map(|usage| {
            let dependency = usage.dependency;
            serde_json::json!({
                "owner": &dependency.owner_root,
                "name": &dependency.dependency.name,
                "scope": format!("{:?}", dependency.dependency.scope),
                "resolved_root": dependency.resolved_root.as_deref(),
                "resolved": usage.resolved,
                "loaded_modules": usage.loaded_modules,
                "used": usage.used,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "build-explanation",
        "plan": build_plan_json_value(plan),
        "package_root": invocation.package_root.to_string_lossy(),
        "package_graph_fingerprint": hex_fingerprint(&invocation.package_source_fingerprint),
        "package_graph": {
            "declared": dependencies,
            "resolved_count": invocation.package_graph_report.resolved.len(),
            "resolved_dependency_count": invocation
                .package_graph_report
                .declared
                .iter()
                .filter(|dependency| dependency.resolved_root.is_some())
                .count(),
        },
        "configure_cache_candidate": configure_candidate.to_string(),
        "observations": configure_observations_json_value(observations),
        "tools": plan_tool_json_values(plan),
        "selected_step": plan.selected_step().or_else(|| plan.default_step()).map(StepKey::name),
        "targets": { "host": plan.host_target().name(), "artifact": plan.artifact_target().name() },
        "configuration": {
            "profile": profile_name(invocation.profile),
            "optimization": optimization_name(invocation.optimization),
            "lto": link_time_optimization_name(invocation.link_time_optimization),
        },
        "closure": {
            "steps": preview.steps.iter().map(|step| format!("{}::{}", step.package().as_str(), step.name())).collect::<Vec<_>>(),
            "actions": actions,
        },
    })
    .to_string()
        + "\n"
}

fn render_configure_cache_json(candidate: ConfigureCacheCandidate, runner_reused: bool) -> String {
    serde_json::json!({
        "kind": "configure-cache",
        "candidate": candidate.to_string(),
        "runner": if runner_reused { "reused" } else { "executed" },
    })
    .to_string()
        + "\n"
}

fn render_configure_observations_json(bytes: &[u8]) -> String {
    let mut value = configure_observations_json_value(bytes);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "kind".to_string(),
            serde_json::Value::String("configure-observations".to_string()),
        );
    }
    value.to_string() + "\n"
}

fn configure_observations_json_value(bytes: &[u8]) -> serde_json::Value {
    match ConfigureManifest::decode(bytes) {
        Ok(manifest) => serde_json::json!({
            "policy": format!("{:?}", manifest.policy()),
            "observations": manifest.observations().iter().map(|observation| serde_json::json!({
                "path": observation.path().to_string_lossy(),
                "kind": observation.kind().to_string(),
                "fingerprint": hex_fingerprint(observation.fingerprint()),
            })).collect::<Vec<_>>(),
            "valid": true,
        }),
        Err(_) => serde_json::json!({
            "policy": "Poison",
            "observations": [],
            "valid": false,
        }),
    }
}

fn action_kind_name(action: &PlanAction) -> &'static str {
    match action.kind {
        ActionKind::CompilerCheck { .. } => "compiler-check",
        ActionKind::CompilerEmit { .. } => "compiler-emit",
        ActionKind::ExternalCommand(_) => "external-command",
        ActionKind::TestExecutable(_) => "test-executable",
        ActionKind::GeneratedFile { .. } => "generated-file",
        ActionKind::InstallArtifact { .. } => "install-artifact",
        ActionKind::Aggregate => "aggregate",
        ActionKind::Uncacheable { .. } => "uncacheable",
    }
}

fn plan_tool_declarations(plan: &BuildPlan) -> Vec<String> {
    let mut tools = plan_tool_json_values(plan)
        .into_iter()
        .filter_map(|tool| tool["display"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    tools.sort();
    tools.dedup();
    tools
}

fn plan_tool_json_values(plan: &BuildPlan) -> Vec<serde_json::Value> {
    let mut tools = Vec::new();
    for action in plan.actions() {
        let command = match &action.kind {
            ActionKind::ExternalCommand(command) | ActionKind::TestExecutable(command) => command,
            _ => continue,
        };
        let value = match &command.program {
            CommandProgram::Path(path) => serde_json::json!({
                "kind": "path",
                "display": path.protocol_path(),
                "path": logical_path_json(path),
            }),
            CommandProgram::Search(name) => serde_json::json!({
                "kind": "search",
                "display": name,
                "name": name,
            }),
        };
        if !tools.iter().any(|existing| existing == &value) {
            tools.push(value);
        }
    }
    tools.sort_by(|left, right| left["display"].as_str().cmp(&right["display"].as_str()));
    tools
}

fn render_action_cache_json(report: &ExecutionReport, plan: &BuildPlan) -> String {
    serde_json::json!({
        "kind": "action-cache",
        "mode": "execution",
        "entries": report.action_cache.iter().map(|entry| serde_json::json!({
            "action": format!("{}::{}", entry.action.package().as_str(), entry.action.name()),
            "outputs": plan
                .actions()
                .iter()
                .find(|action| action.key == entry.action)
                .map(|action| action_outputs_json(plan, action))
                .unwrap_or_default(),
            // Keep the original compact field for existing consumers. The
            // structured decision is the stable machine-readable contract.
            "outcome": format!("{:?}", entry.outcome),
            "decision": action_cache_outcome_json(&entry.outcome),
        })).collect::<Vec<_>>(),
    })
    .to_string()
        + "\n"
}

fn render_action_cache_preview_json(plan: &BuildPlan, preview: &ExecutionReport) -> String {
    serde_json::json!({
        "kind": "action-cache",
        "mode": "dry_run",
        "entries": preview.actions.iter().map(|key| {
            let outputs = plan
                .actions()
                .iter()
                .find(|action| action.key == *key)
                .map(|action| action_outputs_json(plan, action))
                .unwrap_or_default();
            serde_json::json!({
                "action": format!("{}::{}", key.package().as_str(), key.name()),
                "outputs": outputs,
                "outcome": {
                    "status": "not_consulted",
                    "reason": "dry_run",
                    "invalidations": [],
                    "publication": "not_attempted",
                },
            })
        }).collect::<Vec<_>>(),
    })
    .to_string()
        + "\n"
}

fn render_action_cache_preview_summary(plan: &BuildPlan, preview: &ExecutionReport) -> String {
    use std::fmt::Write as _;
    let mut output = String::from("action cache (dry run):\n");
    for key in &preview.actions {
        let outputs = plan
            .actions()
            .iter()
            .find(|action| action.key == *key)
            .map(|action| action_outputs_json(plan, action).len())
            .unwrap_or(0);
        let _ = writeln!(
            output,
            "  {}::{}: not consulted ({} declared output path(s))",
            key.package().as_str(),
            key.name(),
            outputs
        );
    }
    output
}

fn logical_path_json(path: &LogicalPath) -> serde_json::Value {
    let root = match path.root() {
        LogicalPathRoot::Package(package) => {
            serde_json::json!({ "kind": "package", "package": package.as_str() })
        }
        LogicalPathRoot::Build => serde_json::json!({ "kind": "build" }),
        LogicalPathRoot::Cache => serde_json::json!({ "kind": "cache" }),
        LogicalPathRoot::Toolchain => serde_json::json!({ "kind": "toolchain" }),
        LogicalPathRoot::Artifact(artifact) => serde_json::json!({
            "kind": "artifact",
            "package": artifact.package().as_str(),
            "artifact": artifact.name(),
        }),
    };
    serde_json::json!({ "root": root, "path": path.protocol_path() })
}

fn action_json_value(plan: &BuildPlan, action: &PlanAction) -> serde_json::Value {
    let key = format!("{}::{}", action.key.package().as_str(), action.key.name());
    let base = |kind: &'static str, details: serde_json::Value| {
        serde_json::json!({
            "key": key,
            "package": action.key.package().as_str(),
            "name": action.key.name(),
            "kind": kind,
            "resource_class": resource_class_name(action.resource_class()),
            "details": details,
            "outputs": action_outputs_json(plan, action),
        })
    };
    match &action.kind {
        ActionKind::CompilerCheck {
            module,
            target,
            runtime,
            mode,
        } => base(
            "compiler-check",
            serde_json::json!({
                "module": format!("{}::{}", module.package().as_str(), module.name()),
                "target": target.name(),
                "runtime": runtime_name(*runtime),
                "mode": compilation_mode_name(*mode),
            }),
        ),
        ActionKind::CompilerEmit {
            artifact,
            target,
            static_archives,
        } => base(
            "compiler-emit",
            serde_json::json!({
                "artifact": format!("{}::{}", artifact.package().as_str(), artifact.name()),
                "target": target.name(),
                "static_archives": static_archives.iter().map(|archive| format!("{}::{}", archive.package().as_str(), archive.name())).collect::<Vec<_>>(),
            }),
        ),
        ActionKind::ExternalCommand(command) => {
            base("external-command", command_json_value(command))
        }
        ActionKind::TestExecutable(command) => base("test-executable", command_json_value(command)),
        ActionKind::GeneratedFile { output, contents } => base(
            "generated-file",
            serde_json::json!({ "output": logical_path_json(output), "content_bytes": contents.len() }),
        ),
        ActionKind::InstallArtifact {
            artifact,
            destination,
        } => base(
            "install-artifact",
            serde_json::json!({
                "artifact": format!("{}::{}", artifact.package().as_str(), artifact.name()),
                "destination": logical_path_json(destination),
            }),
        ),
        ActionKind::Aggregate => base("aggregate", serde_json::json!({})),
        ActionKind::Uncacheable { description } => base(
            "uncacheable",
            serde_json::json!({ "description": description }),
        ),
    }
}

fn command_json_value(command: &CommandAction) -> serde_json::Value {
    let program = match &command.program {
        CommandProgram::Path(path) => {
            serde_json::json!({ "kind": "path", "path": logical_path_json(path) })
        }
        CommandProgram::Search(name) => serde_json::json!({ "kind": "search", "name": name }),
    };
    serde_json::json!({
        "program": program,
        "stdin": match &command.stdin {
            CommandStdin::Null => serde_json::json!({ "kind": "null" }),
            CommandStdin::InputPath(path) => serde_json::json!({ "kind": "input", "path": logical_path_json(path) }),
        },
        "arguments": command.arguments.iter().map(|argument| match argument {
            CommandArgument::Literal(value) => serde_json::json!({ "kind": "literal", "value": value }),
            CommandArgument::InputPath(path) => serde_json::json!({ "kind": "input", "path": logical_path_json(path) }),
            CommandArgument::OutputPath(path) => serde_json::json!({ "kind": "output", "path": logical_path_json(path) }),
        }).collect::<Vec<_>>(),
        "working_directory": logical_path_json(&command.working_directory),
        "environment_policy": environment_policy_name(command.environment_policy),
        "cache_policy": cache_policy_name(command.cache_policy),
        "environment": command.environment.iter().map(environment_json_value).collect::<Vec<_>>(),
        "inputs": command.inputs.iter().map(logical_path_json).collect::<Vec<_>>(),
        "outputs": command.outputs.iter().map(logical_path_json).collect::<Vec<_>>(),
    })
}

fn environment_json_value(input: &EnvironmentInput) -> serde_json::Value {
    let value = match &input.value {
        Some(value) => {
            let mut builder = QueryFingerprintBuilder::new(REPORT_ENVIRONMENT_VALUE_DOMAIN);
            builder.write_str(&input.name);
            builder.write_str(value);
            let [first, second] = builder.finish().parts();
            serde_json::json!({
                "kind": "explicit",
                "bytes": value.len(),
                "fingerprint": format!("{first:016x}{second:016x}"),
            })
        }
        None => serde_json::json!({ "kind": "inherited" }),
    };
    serde_json::json!({ "name": input.name, "value": value })
}

fn action_outputs_json(plan: &BuildPlan, action: &PlanAction) -> Vec<serde_json::Value> {
    action_output_paths(plan, action)
        .iter()
        .map(logical_path_json)
        .collect()
}

fn action_output_paths<'a>(plan: &'a BuildPlan, action: &'a PlanAction) -> &'a [LogicalPath] {
    match &action.kind {
        ActionKind::CompilerEmit { artifact, .. } => plan
            .artifacts()
            .binary_search_by(|candidate| candidate.key.cmp(artifact))
            .ok()
            .map(|index| std::slice::from_ref(&plan.artifacts()[index].output))
            .unwrap_or_default(),
        ActionKind::ExternalCommand(command) | ActionKind::TestExecutable(command) => {
            &command.outputs
        }
        ActionKind::GeneratedFile { output, .. } => std::slice::from_ref(output),
        ActionKind::InstallArtifact { destination, .. } => std::slice::from_ref(destination),
        ActionKind::CompilerCheck { .. }
        | ActionKind::Aggregate
        | ActionKind::Uncacheable { .. } => &[],
    }
}

fn progress_action_description(plan: &BuildPlan, action: &PlanAction) -> ProgressActionDescription {
    let outputs = action_output_paths(plan, action);
    let module_source = |module: &ModuleKey| {
        plan.modules()
            .binary_search_by(|candidate| candidate.key.cmp(module))
            .ok()
            .map(|index| &plan.modules()[index].root_source)
    };
    let artifact = |key: &ArtifactKey| {
        plan.artifacts()
            .binary_search_by(|candidate| candidate.key.cmp(key))
            .ok()
            .map(|index| &plan.artifacts()[index])
    };
    let (inputs, total_inputs) = match &action.kind {
        ActionKind::ExternalCommand(command) | ActionKind::TestExecutable(command) => (
            command.inputs.iter().take(8).cloned().collect(),
            command.inputs.len(),
        ),
        ActionKind::CompilerCheck { module, .. } => {
            let inputs = module_source(module)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let count = inputs.len();
            (inputs, count)
        }
        ActionKind::CompilerEmit {
            artifact: key,
            static_archives,
            ..
        } => {
            let inputs = artifact(key)
                .and_then(|artifact| module_source(&artifact.root_module))
                .into_iter()
                .chain(
                    static_archives
                        .iter()
                        .filter_map(|key| artifact(key).map(|artifact| &artifact.output)),
                )
                .take(8)
                .cloned()
                .collect();
            (inputs, 1 + static_archives.len())
        }
        ActionKind::InstallArtifact { artifact: key, .. } => {
            let inputs = artifact(key)
                .map(|artifact| artifact.output.clone())
                .into_iter()
                .collect::<Vec<_>>();
            let count = inputs.len();
            (inputs, count)
        }
        _ => (Vec::new(), 0),
    };
    ProgressActionDescription {
        kind: action_kind_name(action),
        inputs,
        total_inputs,
        outputs: outputs.iter().take(8).cloned().collect(),
        total_outputs: outputs.len(),
    }
}

fn action_cache_outcome_json(outcome: &ActionCacheOutcome) -> serde_json::Value {
    match outcome {
        ActionCacheOutcome::Hit => serde_json::json!({
            "status": "hit",
            "reason": serde_json::Value::Null,
            "invalidations": [],
            "publication": "restored",
        }),
        ActionCacheOutcome::Miss(reason) => {
            let publication = match reason {
                ActionCacheMissReason::Uncacheable => "not_applicable",
                ActionCacheMissReason::WriteError => "failed",
                _ => "published",
            };
            serde_json::json!({
                "status": "miss",
                "reason": action_cache_miss_reason_name(reason),
                "invalidations": action_cache_invalidations_json(reason),
                "publication": publication,
            })
        }
    }
}

fn action_cache_miss_reason_name(reason: &ActionCacheMissReason) -> &'static str {
    match reason {
        ActionCacheMissReason::NotFound => "not_found",
        ActionCacheMissReason::Invalidated(_) => "invalidated",
        ActionCacheMissReason::Uncacheable => "uncacheable",
        ActionCacheMissReason::Corrupt => "corrupt",
        ActionCacheMissReason::ReadError => "read_error",
        ActionCacheMissReason::WriteError => "write_error",
    }
}

fn action_cache_invalidations_json(reason: &ActionCacheMissReason) -> Vec<&'static str> {
    match reason {
        ActionCacheMissReason::Invalidated(reasons) => {
            reasons.iter().map(action_cache_invalidation_name).collect()
        }
        _ => Vec::new(),
    }
}

fn action_cache_invalidation_name(reason: &ActionCacheInvalidation) -> &'static str {
    match reason {
        ActionCacheInvalidation::Command => "command",
        ActionCacheInvalidation::ExternalTool => "external_tool",
        ActionCacheInvalidation::Environment => "environment",
        ActionCacheInvalidation::Inputs => "inputs",
        ActionCacheInvalidation::Dependencies => "dependencies",
        ActionCacheInvalidation::WorkingDirectory => "working_directory",
        ActionCacheInvalidation::PackageRoots => "package_roots",
        ActionCacheInvalidation::Contents => "contents",
        ActionCacheInvalidation::Artifact => "artifact",
        ActionCacheInvalidation::Sources => "sources",
        ActionCacheInvalidation::Module => "module",
        ActionCacheInvalidation::Target => "target",
        ActionCacheInvalidation::Profile => "profile",
        ActionCacheInvalidation::CompilationMode => "compilation_mode",
        ActionCacheInvalidation::Optimization => "optimization",
        ActionCacheInvalidation::Runtime => "runtime",
        ActionCacheInvalidation::Linker => "linker",
        ActionCacheInvalidation::Output => "output",
        ActionCacheInvalidation::Compiler => "compiler",
        ActionCacheInvalidation::ResourceLayout => "resource_layout",
        ActionCacheInvalidation::StandardLibrary => "standard_library",
        ActionCacheInvalidation::BuildProtocol => "build_protocol",
    }
}

fn profile_name(value: BuildProfile) -> &'static str {
    match value {
        BuildProfile::Debug => "debug",
        BuildProfile::Release => "release",
    }
}

fn link_time_optimization_name(value: LinkTimeOptimization) -> &'static str {
    match value {
        LinkTimeOptimization::Off => "off",
        LinkTimeOptimization::Thin => "thin",
        LinkTimeOptimization::Full => "full",
    }
}

fn optimization_name(value: OptimizationMode) -> &'static str {
    match value {
        OptimizationMode::O0 => "O0",
        OptimizationMode::O1 => "O1",
        OptimizationMode::O2 => "O2",
        OptimizationMode::O3 => "O3",
        OptimizationMode::Os => "Os",
        OptimizationMode::Oz => "Oz",
    }
}

fn runtime_name(value: Runtime) -> &'static str {
    match value {
        Runtime::Bare => "bare",
        Runtime::Freestanding => "freestanding",
    }
}

fn compilation_mode_name(value: CompilationMode) -> &'static str {
    match value {
        CompilationMode::Normal => "normal",
        CompilationMode::Test => "test",
    }
}

fn artifact_kind_name(value: &PlanArtifactKind) -> &'static str {
    match value {
        PlanArtifactKind::Executable => "executable",
        PlanArtifactKind::ObjectSet => "object_set",
        PlanArtifactKind::StaticArchive => "static_archive",
    }
}

fn resource_class_name(value: ActionResourceClass) -> &'static str {
    match value {
        ActionResourceClass::Conservative => "conservative",
        ActionResourceClass::Cpu => "cpu",
        ActionResourceClass::Io => "io",
    }
}

fn environment_policy_name(value: CommandEnvironmentPolicy) -> &'static str {
    match value {
        CommandEnvironmentPolicy::Inherit => "inherit",
        CommandEnvironmentPolicy::Clear => "clear",
    }
}

fn cache_policy_name(value: CommandCachePolicy) -> &'static str {
    match value {
        CommandCachePolicy::Uncacheable => "uncacheable",
        CommandCachePolicy::DeclaredInputs => "declared_inputs",
        CommandCachePolicy::ToolAndInputs => "tool_and_inputs",
        CommandCachePolicy::FullDeclaredEnvironment => "full_declared_environment",
        CommandCachePolicy::SuccessfulTests => "successful_tests",
    }
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
                                ActionCacheInvalidation::Profile => nia_timing::emit_counter(
                                    "build.action_cache_invalidation_profile",
                                    1,
                                ),
                                ActionCacheInvalidation::CompilationMode => {
                                    nia_timing::emit_counter(
                                        "build.action_cache_invalidation_compilation_mode",
                                        1,
                                    )
                                }
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

/// Resolves package paths and invocation-private directories without running actions.
pub fn resolve_build_invocation(request: BuildRequest) -> Result<BuildInvocation, BuildError> {
    let start = match request.root {
        Some(root) if root.is_absolute() => root,
        Some(root) => env::current_dir()
            .map_err(|error| BuildError::CurrentDirectory { error })?
            .join(root),
        None => env::current_dir().map_err(|error| BuildError::CurrentDirectory { error })?,
    };
    let layout = match nia_package_metadata::BuildLayout::discover(&start) {
        Ok(layout) => layout,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(BuildError::MissingBuildScript { start });
        }
        Err(error) => return Err(BuildError::ResolvePackageGraph { path: start, error }),
    };
    let package_root = normalize_host_path(layout.package_root().to_path_buf());

    let package_graph =
        nia_package_metadata::PackageGraph::resolve_with_policy(&package_root, request.lock_policy)
            .map_err(|error| BuildError::ResolvePackageGraph {
                path: package_root.clone(),
                error,
            })?;
    let build_script = package_root.join("build.nia");
    let manifest_artifacts = if package_root.join("nia.toml").is_file() {
        nia_package_metadata::PackageManifest::read(&package_root)
            .map_err(|error| BuildError::ResolvePackageGraph {
                path: package_root.clone(),
                error,
            })?
            .artifacts
    } else {
        Vec::new()
    };

    let build_dir = normalize_host_path(layout.out_dir());
    let cache_dir = normalize_host_path(layout.cache_dir());
    let tmp_dir = normalize_host_path(layout.tmp_dir());
    let transient_name = next_build_invocation_name();
    let artifact_cache_dir = shared_artifact_cache_dir().unwrap_or_else(|| cache_dir.clone());
    Ok(BuildInvocation {
        toolchain: request.toolchain,
        cache_dir,
        artifact_cache_dir,
        runner_executable: tmp_dir.join(format!("nia-build-runner-{transient_name}")),
        runner_config: tmp_dir.join(format!("build-runner-{transient_name}.config")),
        plan_draft: tmp_dir.join(format!("build-plan-{transient_name}.draft")),
        plan_path: build_dir.join("build-plan.bin"),
        tmp_dir,
        build_dir,
        package_root,
        package_source_fingerprint: package_graph.fingerprint(),
        package_graph_report: package_graph.report(),
        build_script,
        manifest_artifacts,
        step: if request.list {
            BuildStepSelection::List
        } else {
            request
                .step
                .map(BuildStepSelection::Named)
                .unwrap_or(BuildStepSelection::Default)
        },
        keep_going: request.keep_going,
        forward_output: request.forward_output,
        timings: request.timings,
        timing_format: request.timing_format,
        max_parallel_actions: request.max_parallel_actions,
        optimization: request.optimization,
        profile: request.profile,
        link_time_optimization: request.link_time_optimization,
    })
}

/// Names a compiler-cache root shared by every build in one test run, so each
/// test package does not compile the same runner and standard library again.
/// Only `nia-test-support` sets it; it is not a user-facing option.
pub const TEST_SHARED_ARTIFACT_CACHE_ENV: &str = "NIA_TEST_SHARED_ARTIFACT_CACHE";

fn shared_artifact_cache_dir() -> Option<PathBuf> {
    env::var_os(TEST_SHARED_ARTIFACT_CACHE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

static BUILD_INVOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const BUILD_RUNNER_SOURCE_PATH: &str = "build-package:build-runner:/root.nia";
const BUILD_RUNNER_OPTIMIZATION: nia_driver::NiaOptimizationLevel =
    nia_driver::NiaOptimizationLevel::O0;
// The wrapper is Nia code; Rust only embeds the template and injects the
// invocation-specific protocol identities before compiling it.
const BUILD_RUNNER_TEMPLATE: &str = include_str!("runner.nia");

fn next_build_invocation_name() -> String {
    let sequence = BUILD_INVOCATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence}", std::process::id())
}

/// Produces the generated host-runner source for a resolved invocation.
pub fn build_runner_source(invocation: &BuildInvocation) -> Result<BuildRunnerSource, BuildError> {
    build_runner_source_for_path(invocation, BUILD_RUNNER_SOURCE_PATH.to_string())
}

fn build_runner_source_for_path(
    invocation: &BuildInvocation,
    path: String,
) -> Result<BuildRunnerSource, BuildError> {
    let _ = invocation;
    let source = BUILD_RUNNER_TEMPLATE.to_owned();
    let source = source
        .replace(
            "__NIA_RUNNER_CONFIG_PROTOCOL_EPOCH__",
            &RUNNER_CONFIG.protocol_epoch.to_string(),
        )
        .replace(
            "__NIA_BUILD_PROTOCOL_EPOCH__",
            &nia_compat::formats::BUILD_PLAN.protocol_epoch.to_string(),
        )
        .replace(
            "__NIA_RUNNER_CONFIG_MAX_BYTES__",
            &runner_config::RUNNER_CONFIG_MAX_BYTES.to_string(),
        )
        .replace(
            "__NIA_TARGET_COUNT__",
            &nia_target::SUPPORTED_TARGETS.len().to_string(),
        )
        .replace(
            "__NIA_RUNNER_CONFIG_MAGIC__",
            std::str::from_utf8(RUNNER_CONFIG.magic).map_err(|_| {
                BuildError::PrepareRunnerConfiguration {
                    path: invocation.runner_config.clone(),
                    error: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runner config magic is not valid UTF-8",
                    ),
                }
            })?,
        );
    Ok(BuildRunnerSource { path, source })
}

fn compile_build_runner(
    invocation: &BuildInvocation,
    progress: Option<&ProgressSession>,
) -> Result<(PathBuf, String), BuildError> {
    let lock_observer =
        progress.map(|progress| progress.phase_lock_observer(ProgressPhase::Runner));
    let runner = time_build_stage(invocation.timings, "build_runner_generate_source", || {
        build_runner_source(invocation)
    })?;
    if invocation.timings.enabled() {
        nia_timing::emit_counter("build.runner_source_bytes", runner.source.len() as u64);
        nia_timing::emit_counter(
            "build.runner_source_lines",
            runner.source.lines().count() as u64,
        );
    }
    let cache_key = time_build_stage(invocation.timings, "build_runner_compute_cache_key", || {
        runner_cache::cache_key(invocation, &runner)
    })?;
    let restored = time_build_stage(invocation.timings, "build_runner_restore_cache", || {
        runner_cache::restore_executable(invocation, &cache_key, lock_observer.as_ref())
            .unwrap_or(false)
    });
    if let Some(progress) = progress {
        progress.emit(ProgressEventKind::PhaseCacheDecision {
            phase: ProgressPhase::Runner,
            reused: restored,
        });
    }
    if restored {
        nia_timing::emit_counter("build.runner_cache_hits", 1);
        return Ok((invocation.runner_executable.clone(), cache_key));
    }
    nia_timing::emit_counter("build.runner_cache_misses", 1);
    if let Some(parent) = invocation.runner_executable.parent() {
        create_real_directory(parent).map_err(|error| BuildError::CreateRunnerDirectory {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    let driver = time_build_stage(invocation.timings, "build_runner_create_driver", || {
        Driver::with_config(build_runner_driver_config(invocation))
    });
    if !invocation.build_script.is_file() {
        driver
            .set_source(EMPTY_BUILD_SCRIPT_PATH, EMPTY_BUILD_SCRIPT)
            .map_err(|error| BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(error),
            })?;
    }
    time_build_stage(invocation.timings, "build_runner_set_source", || {
        driver
            .set_source(runner.path.clone(), runner.source.clone())
            .map_err(|error| BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(error),
            })
    })?;
    let module_map = time_build_stage(invocation.timings, "build_runner_build_module_map", || {
        build_runner_module_map(invocation).map_err(|error| match error {
            BuildRunnerModuleMapError::InvalidRoot { name, reason } => {
                BuildError::InvalidBuildModuleRoot { name, reason }
            }
            BuildRunnerModuleMapError::Manifest(error) => BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(DriverError::InternalDiagnostic(
                    nia_diagnostic::Diagnostic::from(nia_ice::Ice::new(error.to_string())),
                )),
            },
        })
    })?;
    let check = CheckRequest::new(runner.path.clone())
        .with_module_map(module_map)
        .with_profile(invocation.profile)
        .with_optimization(BUILD_RUNNER_OPTIMIZATION)
        .with_timings(invocation.timings);
    let runtime = nia_toolchain::RuntimeSpec::freestanding(
        &invocation.toolchain,
        &invocation.toolchain.host_target().clone(),
    )
    .map_err(|error| BuildError::CompileRunner {
        path: runner.path.clone(),
        source: runner.source.clone(),
        error: Box::new(crate::DriverError::Runtime(error)),
    })?;
    let check = check
        .with_runtime(runtime)
        .with_current_package(runner_cache::package_id(&cache_key));
    let runtime_entry = check
        .runtime
        .source()
        .map(|runtime| runtime.entry_point().linker_symbol().to_owned());
    // Check the user-owned build script before lowering the generated wrapper.
    // A bad build script otherwise poisons the wrapper's recovery values and
    // produces a long list of diagnostics at synthetic runner locations.
    if let Err(error) = driver.check_entry(check.clone()).result {
        let error = focus_build_script_diagnostics(error, &SourcePath::new(runner.path.clone()));
        return Err(BuildError::CompileRunner {
            path: runner.path.clone(),
            source: runner.source.clone(),
            error: Box::new(error),
        });
    }
    let output = match invocation.link_time_optimization {
        LinkTimeOptimization::Off => {
            let objects = time_build_stage(
                invocation.timings,
                "build_runner_emit_native_objects",
                || driver.emit_native_objects(EmitObjectRequest::new(check)),
            );
            let objects = objects.result.map_err(|error| BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(error),
            })?;
            time_build_stage(invocation.timings, "build_runner_link_executable", || {
                driver.link_executable_from_objects(
                    &objects,
                    invocation.runner_executable.clone(),
                    nia_linker::LinkOptions {
                        entry: runtime_entry,
                        ..nia_linker::LinkOptions::default()
                    },
                    invocation.timings,
                )
            })
        }
        policy => time_build_stage(invocation.timings, "build_runner_link_executable", || {
            driver.link_executable(
                LinkExecutableRequest::new(check, invocation.runner_executable.clone())
                    .with_link_time_optimization(policy),
            )
        }),
    };
    output.result.map_err(|error| BuildError::CompileRunner {
        path: runner.path.clone(),
        source: runner.source.clone(),
        error: Box::new(error),
    })?;
    let publish_result = time_build_stage(invocation.timings, "build_runner_publish_cache", || {
        runner_cache::publish_executable(invocation, &cache_key, lock_observer.as_ref())
    });
    if publish_result.is_err() {
        nia_timing::emit_counter("build.runner_cache_publish_failures", 1);
    }
    Ok((invocation.runner_executable.clone(), cache_key))
}

fn focus_build_script_diagnostics(error: DriverError, runner: &SourcePath) -> DriverError {
    match error {
        DriverError::CheckDiagnostics(mut program) => {
            focus_program_diagnostics(&mut program.diagnostics, runner);
            DriverError::CheckDiagnostics(program)
        }
        DriverError::CodegenProgramDiagnostics(mut program) => {
            focus_program_diagnostics(&mut program.diagnostics, runner);
            DriverError::CodegenProgramDiagnostics(program)
        }
        DriverError::CodegenPreparationDiagnostics {
            diagnostic_sources,
            mut diagnostics,
            suppressed_downstream,
        } => {
            focus_program_diagnostics(&mut diagnostics, runner);
            DriverError::CodegenPreparationDiagnostics {
                diagnostic_sources,
                diagnostics,
                suppressed_downstream,
            }
        }
        error => error,
    }
}

fn focus_program_diagnostics(diagnostics: &mut Vec<ProgramDiagnostic>, runner: &SourcePath) {
    let has_source_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.is_error() && diagnostic.path != *runner);
    if has_source_error {
        // Preserve dependency errors and warnings. Only the generated wrapper
        // is recovery context, and an explicit cause is required to attach it.
        let context = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.path == *runner)
            .filter_map(|diagnostic| {
                Some((
                    diagnostic.diagnostic.cause.as_deref()?.clone(),
                    nia_diagnostic::RelatedDiagnostic {
                        source_path: Some(runner.as_str().to_owned()),
                        span: diagnostic.diagnostic.primary_span()?,
                        message: format!(
                            "generated runner context: {}",
                            diagnostic.diagnostic.summary
                        ),
                    },
                ))
            })
            .collect::<Vec<_>>();
        diagnostics.retain(|diagnostic| diagnostic.path != *runner);
        for (cause, related) in context {
            let mut roots = diagnostics.iter_mut().filter(|diagnostic| {
                diagnostic.path.as_str() == cause.source_path
                    && diagnostic.diagnostic.code.as_str() == cause.code
                    && diagnostic.diagnostic.primary_span() == Some(cause.span)
                    && diagnostic.diagnostic.cause.is_none()
            });
            if let Some(root) = roots.next()
                && roots.next().is_none()
                && !root.diagnostic.related.contains(&related)
            {
                root.diagnostic.related.push(related);
            }
        }
    }
}

fn build_runner_driver_config(invocation: &BuildInvocation) -> DriverConfig {
    DriverConfig {
        artifact_cache_dir: Some(invocation.artifact_cache_dir.clone()),
        ..DriverConfig::new(Arc::clone(&invocation.toolchain))
    }
    .with_artifact_target(*invocation.toolchain.host_target())
}

#[derive(Debug)]
enum BuildRunnerModuleMapError {
    InvalidRoot { name: String, reason: String },
    Manifest(io::Error),
}

fn build_runner_module_map(
    invocation: &BuildInvocation,
) -> Result<ModuleMap, BuildRunnerModuleMapError> {
    let mut module_map = ModuleMap::new();
    module_map
        .insert("buildScript", build_script_source_path(invocation))
        .map_err(|error| BuildRunnerModuleMapError::InvalidRoot {
            name: "buildScript".to_owned(),
            reason: error,
        })?;
    add_manifest_module_roots(
        &mut module_map,
        &invocation.package_graph_report,
        &[DependencyScope::Build],
        CompilationMode::Normal,
    )
    .map_err(BuildRunnerModuleMapError::Manifest)?;
    Ok(module_map)
}

/// `build.nia`, or, when the package has none, the toolchain's empty script.
fn build_script_source_path(invocation: &BuildInvocation) -> SourcePath {
    if invocation.build_script.is_file() {
        SourcePath::with_identity(
            invocation.build_script.to_string_lossy().into_owned(),
            "build-package:root:/build.nia",
        )
    } else {
        SourcePath::with_identity(EMPTY_BUILD_SCRIPT_PATH, EMPTY_BUILD_SCRIPT_PATH)
    }
}

// Declares nothing beyond the manifest artifacts the runner adds first.
const EMPTY_BUILD_SCRIPT_PATH: &str = "build-package:build-runner:/empty-build.nia";
const EMPTY_BUILD_SCRIPT: &str = "using std::build;\n\n\
pub fun build(b: &mut build::Build) build::Error!() {\n    _ = b;\n    !()\n}\n";

pub(crate) fn manifest_module_roots(
    report: &nia_package_metadata::PackageGraphReport,
    scopes: &[DependencyScope],
    compilation_mode: CompilationMode,
) -> io::Result<Vec<(String, String, SourcePath)>> {
    let mut imports = Vec::new();
    for edge in &report.declared {
        let selected = match edge.dependency.scope {
            DependencyScope::Build => scopes.contains(&DependencyScope::Build),
            DependencyScope::Runtime => scopes.contains(&DependencyScope::Runtime),
            DependencyScope::Development => {
                scopes.contains(&DependencyScope::Development)
                    && compilation_mode == CompilationMode::Test
            }
            DependencyScope::Optional => scopes.contains(&DependencyScope::Runtime),
            DependencyScope::Lazy => scopes.contains(&DependencyScope::Runtime),
        };
        let Some(logical_root) = edge.resolved_root.as_deref() else {
            continue;
        };
        if !selected {
            continue;
        }
        let Some(package_root) = &edge.source_root else {
            continue;
        };
        let manifest = nia_package_metadata::PackageManifest::read(package_root)?;
        let Some(library_path) = manifest.library_path else {
            continue;
        };
        let path = package_root.join(&library_path);
        imports.push((
            edge.owner_root.clone(),
            edge.dependency.name.clone(),
            SourcePath::with_identity(
                path.to_string_lossy().into_owned(),
                format!("build-package:{logical_root}:/{library_path}"),
            ),
        ));
    }
    Ok(imports)
}

fn package_module_scope(logical_root: &str) -> SourceIdentity {
    let logical_root = if logical_root.is_empty() {
        "root"
    } else {
        logical_root
    };
    SourceIdentity::new(format!("build-package:{logical_root}:/"))
}

pub(crate) fn add_manifest_module_roots(
    module_map: &mut ModuleMap,
    report: &nia_package_metadata::PackageGraphReport,
    scopes: &[DependencyScope],
    compilation_mode: CompilationMode,
) -> io::Result<()> {
    for node in &report.resolved {
        module_map.add_scope(&package_module_scope(&node.root));
    }
    for (owner, name, path) in manifest_module_roots(report, scopes, compilation_mode)? {
        module_map
            .insert_scoped(&package_module_scope(&owner), &name, path.clone())
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidData, reason))?;
    }
    Ok(())
}

/// Lets the root package's artifacts open the package's own library by the
/// package name, the way a dependent package would. The library is its own
/// module tree: an executable that imports it sees only the library's `pub`
/// surface, never its `pub(root)` items.
pub(crate) fn add_own_library_module_root(
    module_map: &mut ModuleMap,
    report: &nia_package_metadata::PackageGraphReport,
    package_root: &Path,
) -> io::Result<()> {
    if !package_root.join("nia.toml").is_file() {
        return Ok(());
    }
    let manifest = nia_package_metadata::PackageManifest::read(package_root)?;
    let Some(library_path) = manifest.library_path else {
        return Ok(());
    };
    let name = &manifest.package.name;
    if report
        .declared
        .iter()
        .any(|edge| edge.owner_root.is_empty() && edge.dependency.name == *name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "dependency `{name}` has the same name as the package; the package's own library already uses it"
            ),
        ));
    }
    let path = SourcePath::with_identity(
        package_root
            .join(&library_path)
            .to_string_lossy()
            .into_owned(),
        format!("build-package:root:/{library_path}"),
    );
    module_map
        .insert_scoped(&package_module_scope(""), name, path)
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidData, reason))
}

pub(crate) fn insert_module_root(
    module_map: &mut ModuleMap,
    name: &str,
    path: SourcePath,
) -> Result<(), String> {
    if let Some(existing) = module_map.get(name) {
        return if existing == &path {
            Ok(())
        } else {
            Err(format!(
                "package root `{name}` resolves to multiple package instances in the global module namespace"
            ))
        };
    }
    module_map.try_insert(name, path)
}

fn validate_manifest_dependency_references(
    packages: &[PlanPackage],
    invocation: &BuildInvocation,
) -> Result<(), String> {
    for package in packages {
        let PlanPackageSource::Dependency(name) = &package.source else {
            continue;
        };
        let Some(dependency) = invocation
            .package_graph_report
            .declared
            .iter()
            .find(|dependency| {
                dependency.owner_root.is_empty() && dependency.dependency.name == *name
            })
        else {
            return Err(format!("`{name}` is not declared in the root nia.toml"));
        };
        match dependency.dependency.scope {
            // Only test artifacts see a development dependency; the
            // coordinator hides it from ordinary compilations.
            DependencyScope::Runtime | DependencyScope::Development => {}
            DependencyScope::Optional | DependencyScope::Lazy => {}
            DependencyScope::Build => {
                return Err(format!(
                    "`{name}` is build-only; build.nia can import it directly without addDependency"
                ));
            }
        }
        if dependency.resolved_root.is_none() || dependency.source_root.is_none() {
            return Err(format!(
                "`{name}` is not present in the selected package graph"
            ));
        }
    }
    Ok(())
}

fn prepare_build_directories(invocation: &BuildInvocation) -> Result<(), BuildError> {
    create_real_directory(&invocation.build_dir).map_err(|error| {
        BuildError::CreateBuildDirectory {
            path: invocation.build_dir.clone(),
            error,
        }
    })?;
    create_real_directory(&invocation.tmp_dir).map_err(|error| {
        BuildError::CreateRunnerDirectory {
            path: invocation.tmp_dir.clone(),
            error,
        }
    })?;
    create_real_directory(&invocation.cache_dir).map_err(|error| BuildError::CreateCacheDirectory {
        path: invocation.cache_dir.clone(),
        error,
    })
}

fn run_build_runner(
    invocation: &BuildInvocation,
    runner_executable: &Path,
    runner_cache_key: &str,
    process_output_writer: Option<Arc<dyn ProcessOutputWriter>>,
    progress: Option<&ProgressSession>,
) -> Result<BuildDraft, BuildError> {
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
    time_build_stage(
        invocation.timings,
        "build_runner_prepare_configuration",
        || prepare_runner_configuration(invocation),
    )?;
    let mut command = Command::new(runner_executable);
    // Probes and package paths in build.nia are relative to the package root.
    command
        .current_dir(&invocation.package_root)
        .arg("--config")
        .arg(&invocation.runner_config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_process_group(&mut command);
    let result = match time_build_stage(invocation.timings, "build_runner_execute_process", || {
        execute_runner_process(
            &mut command,
            invocation.forward_output,
            process_output_writer,
        )
    }) {
        Ok(output) if output.status.success() => {
            let draft = time_build_stage(invocation.timings, "build_runner_read_plan", || {
                read_build_draft(&invocation.plan_draft)
            });
            match draft {
                Ok(mut draft) => {
                    let plan = draft.plan.clone();
                    let mut manifest = if draft.observations.is_empty() {
                        ConfigureManifest::new(ObservationPolicy::Disallow)
                    } else {
                        decode_and_validate_runner_observations(
                            &invocation.package_root,
                            &draft.observations,
                        )
                        .map_err(|error| {
                            BuildError::RunnerObservations {
                                path: invocation.plan_draft.clone(),
                                error,
                            }
                        })?
                    };
                    if manifest.observations().is_empty()
                        && let Ok(observation) =
                            observe_file_contents(&invocation.package_root, "build.nia")
                    {
                        manifest
                            .push(observation)
                            .map_err(|_| BuildError::RunnerObservations {
                                path: invocation.plan_draft.clone(),
                                error: io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "duplicate configure observation",
                                ),
                            })?;
                    }
                    draft.observations =
                        manifest
                            .encode()
                            .map_err(|error| BuildError::RunnerObservations {
                                path: invocation.plan_draft.clone(),
                                error: io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!("observation manifest encoding failed: {error:?}"),
                                ),
                            })?;
                    if !draft.observations.is_empty() {
                        nia_timing::emit_counter(
                            "build.configure_observations_bytes",
                            draft.observations.len() as u64,
                        );
                    }
                    time_build_stage(invocation.timings, "build_runner_publish_plan", || {
                        publish_build_plan(&invocation.plan_path, &plan)
                            .map(|()| {
                                let manifest = if draft.observations.is_empty() {
                                    Some(ConfigureManifest::new(ObservationPolicy::Poison))
                                } else {
                                    ConfigureManifest::decode(&draft.observations).ok()
                                };
                                if let Some(manifest) = manifest {
                                    let identity = ConfigureIdentity::new(
                                        runner_cache_key,
                                        invocation_identity(invocation),
                                        &manifest,
                                    );
                                    if ConfigureCache::new(invocation)
                                        .with_lock_observer(progress.map(|progress| {
                                            progress.phase_lock_observer(ProgressPhase::Configure)
                                        }))
                                        .publish_draft(&identity, &draft)
                                        .is_ok()
                                    {
                                        nia_timing::emit_counter(
                                            "build.configure_cache_publications",
                                            1,
                                        );
                                    } else {
                                        nia_timing::emit_counter(
                                            "build.configure_cache_failures",
                                            1,
                                        );
                                    }
                                }
                                draft
                            })
                            .map_err(|error| BuildError::PublishBuildPlan {
                                path: invocation.plan_path.clone(),
                                error,
                            })
                    })
                }
                Err(error) => Err(error),
            }
        }
        Ok(output) => Err(BuildError::RunnerFailed {
            path: runner_executable.to_path_buf(),
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        }),
        Err(error) => Err(BuildError::RunRunner {
            path: runner_executable.to_path_buf(),
            error: error.error,
            stdout: error.stdout,
            stderr: error.stderr,
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

fn read_build_draft(path: &Path) -> Result<BuildDraft, BuildError> {
    let bytes = fs::read(path).map_err(|error| BuildError::ReadPlanDraft {
        path: path.to_path_buf(),
        error: PlanHandoffError::Read {
            path: path.to_path_buf(),
            error,
        },
    })?;
    BuildDraft::decode(&bytes).map_err(|error| BuildError::ReadPlanDraft {
        path: path.to_path_buf(),
        error: PlanHandoffError::Decode {
            path: path.to_path_buf(),
            error: match error {
                BuildDraftError::Plan(error) => error,
                BuildDraftError::TooLarge | BuildDraftError::InvalidEnvelope => {
                    PlanCodecError::BadMagic
                }
            },
        },
    })
}

struct RunnerProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug)]
struct RunnerProcessError {
    error: io::Error,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl From<io::Error> for RunnerProcessError {
    fn from(error: io::Error) -> Self {
        Self {
            error,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

fn execute_runner_process(
    command: &mut Command,
    forward_output: bool,
    output_writer: Option<Arc<dyn ProcessOutputWriter>>,
) -> Result<RunnerProcessOutput, RunnerProcessError> {
    let mut child = command
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("spawn failed: {error}")))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_runner(&mut child);
        io::Error::other("build runner stdout pipe was not created")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_runner(&mut child);
        io::Error::other("build runner stderr pipe was not created")
    })?;
    let stdout_reader = match spawn_runner_capture(
        stdout,
        CapturedStream::Stdout,
        "stdout",
        forward_output,
        output_writer.clone(),
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_runner(&mut child);
            return Err(error.into());
        }
    };
    let stderr_reader = match spawn_runner_capture(
        stderr,
        CapturedStream::Stderr,
        "stderr",
        forward_output,
        output_writer,
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_runner(&mut child);
            let stdout = join_capture_tail(stdout_reader);
            return Err(RunnerProcessError {
                error,
                stdout,
                stderr: Vec::new(),
            });
        }
    };
    let status = match child.wait() {
        Ok(status) => {
            terminate_process_descendants(child.id());
            Ok(status)
        }
        Err(error) => {
            terminate_runner(&mut child);
            Err(error)
        }
    };
    finish_runner_process(status, stdout_reader, stderr_reader)
}

fn finish_runner_process(
    status: io::Result<ExitStatus>,
    stdout_reader: thread::JoinHandle<StreamCapture>,
    stderr_reader: thread::JoinHandle<StreamCapture>,
) -> Result<RunnerProcessOutput, RunnerProcessError> {
    let output = join_captures(stdout_reader, stderr_reader);
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return Err(RunnerProcessError {
                error: io::Error::new(error.kind(), format!("wait failed: {error}")),
                stdout: output.stdout,
                stderr: output.stderr,
            });
        }
    };
    if let Some(failure) = output.failure {
        let error = match failure {
            CaptureFailure::Thread { stream } => {
                io::Error::other(format!("{stream} capture worker failed"))
            }
            CaptureFailure::Io { stream, error } => io::Error::new(
                error.kind(),
                format!("{stream} capture/forward failed: {error}"),
            ),
        };
        return Err(RunnerProcessError {
            error,
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    Ok(RunnerProcessOutput {
        status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn spawn_runner_capture(
    reader: impl io::Read + Send + 'static,
    stream: CapturedStream,
    name: &'static str,
    forward_output: bool,
    output_writer: Option<Arc<dyn ProcessOutputWriter>>,
) -> io::Result<thread::JoinHandle<StreamCapture>> {
    thread::Builder::new()
        .name(format!("nia-build-runner-{name}"))
        .spawn(move || {
            capture_stream(
                reader,
                stream,
                forward_output,
                RUNNER_OUTPUT_TAIL_BYTES,
                output_writer,
            )
        })
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to start {name} capture worker: {error}"),
            )
        })
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

#[cfg(test)]
fn run_and_cleanup_build_runner(
    invocation: &BuildInvocation,
    runner_executable: &Path,
    runner_cache_key: &str,
) -> Result<BuildPlan, BuildError> {
    run_and_cleanup_build_runner_draft(invocation, runner_executable, runner_cache_key, None, None)
        .map(|draft| draft.plan)
}

fn run_and_cleanup_build_runner_draft(
    invocation: &BuildInvocation,
    runner_executable: &Path,
    runner_cache_key: &str,
    process_output_writer: Option<Arc<dyn ProcessOutputWriter>>,
    progress: Option<&ProgressSession>,
) -> Result<BuildDraft, BuildError> {
    let result = time_summary_stage(invocation.timings, "build_run_runner", || {
        run_build_runner(
            invocation,
            runner_executable,
            runner_cache_key,
            process_output_writer,
            progress,
        )
    });
    let cleanup = match fs::remove_file(runner_executable) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BuildError::CleanupRunnerExecutable {
            path: runner_executable.to_path_buf(),
            error,
        }),
    };
    match result {
        Ok(draft) => cleanup.map(|()| draft),
        Err(error) => Err(error),
    }
}

fn time_summary_stage<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_stage(timings, nia_timing::TimingLevel::Summary, name, f)
}

fn time_build_stage<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_stage(timings, nia_timing::TimingLevel::Detail, name, f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static SSH_ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

    /// Points the SSH client's home lookup at a fixture directory and hides
    /// any agent, restoring both on drop, including when the test panics.
    /// The client reads `HOME` on Unix and `USERPROFILE` on Windows.
    struct SshHomeEnvironment {
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl SshHomeEnvironment {
        fn redirect(home: &Path) -> Self {
            let absent_agent = home.join("absent-agent");
            let mut previous = Vec::new();
            for (name, value) in [
                ("HOME", home),
                ("USERPROFILE", home),
                ("SSH_AUTH_SOCK", absent_agent.as_path()),
            ] {
                previous.push((name, env::var_os(name)));
                // SAFETY: SSH_ENVIRONMENT_LOCK serializes the tests that
                // change these variables.
                unsafe { env::set_var(name, value) };
            }
            Self { previous }
        }
    }

    impl Drop for SshHomeEnvironment {
        fn drop(&mut self) {
            for (name, value) in self.previous.drain(..) {
                // SAFETY: see `redirect`.
                match value {
                    Some(value) => unsafe { env::set_var(name, value) },
                    None => unsafe { env::remove_var(name) },
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn package_checkout_paths_drop_windows_extended_prefixes() {
        assert_eq!(
            normalize_host_path(PathBuf::from(r"\\?\C:\packages\json")),
            PathBuf::from(r"C:\packages\json")
        );
        assert_eq!(
            normalize_host_path(PathBuf::from(r"\\?\UNC\server\share\json")),
            PathBuf::from(r"\\server\share\json")
        );
    }

    #[test]
    fn build_reports_redact_declared_environment_values() {
        let explicit = environment_json_value(&EnvironmentInput {
            name: "NIA_SECRET".to_string(),
            value: Some("super-secret-value".to_string()),
        });
        let encoded = explicit.to_string();
        assert_eq!(explicit["name"], "NIA_SECRET");
        assert_eq!(explicit["value"]["kind"], "explicit");
        assert_eq!(explicit["value"]["bytes"], 18);
        assert!(explicit["value"]["fingerprint"].as_str().is_some());
        assert!(!encoded.contains("super-secret-value"), "{encoded}");

        let inherited = environment_json_value(&EnvironmentInput {
            name: "NIA_INHERITED".to_string(),
            value: None,
        });
        assert_eq!(inherited["value"]["kind"], "inherited");
        assert!(inherited["value"].get("fingerprint").is_none());
    }

    #[test]
    fn runner_completion_retains_output_and_failure_stage() {
        use crate::process_output::tests::{capture_worker, success_status};
        for phase in ["stdout", "stderr", "wait", "worker", "success"] {
            let stdout = if phase == "worker" {
                std::thread::spawn(|| panic!("injected runner capture panic"))
            } else {
                capture_worker(CapturedStream::Stdout, phase == "stdout")
            };
            let stderr = capture_worker(CapturedStream::Stderr, phase == "stderr");
            let status = if phase == "wait" {
                Err(io::Error::other("injected wait failure"))
            } else {
                Ok(success_status())
            };
            let error = match finish_runner_process(status, stdout, stderr) {
                Ok(output) => {
                    assert_eq!(phase, "success");
                    assert!(output.status.success());
                    assert_eq!(output.stdout, b"stdout-tail");
                    assert_eq!(output.stderr, b"stderr-tail");
                    continue;
                }
                Err(error) => error,
            };
            let reason = match phase {
                "stdout" | "stderr" => {
                    format!("{phase} capture/forward failed: injected read failure")
                }
                "wait" => "wait failed: injected wait failure".into(),
                "worker" => "stdout capture worker failed".into(),
                _ => unreachable!(),
            };
            let error = BuildError::RunRunner {
                path: PathBuf::from("runner"),
                error: error.error,
                stdout: error.stdout,
                stderr: error.stderr,
            };
            let mut markers = vec![reason.as_str(), "stderr-tail"];
            if phase != "worker" {
                markers.push("stdout-tail");
            }
            crate::report::tests::assert_operational_report(&error, "E0703", &markers);
            let display = error.to_string();
            for marker in markers {
                assert!(display.contains(marker), "{display}");
            }
        }
    }

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

    #[test]
    fn build_script_errors_hide_generated_runner_cascade() {
        let build_script = PathBuf::from("/workspace/build.nia");
        let mut diagnostics = vec![
            ProgramDiagnostic {
                path: SourcePath::new(build_script.to_string_lossy().into_owned()),
                diagnostic: nia_diagnostic::Diagnostic::user_error(
                    nia_diagnostic::codes::NAME_RESOLUTION,
                    "unknown value `exit`",
                )
                .finish(),
            },
            ProgramDiagnostic {
                path: SourcePath::new("build-package:build-runner:/root.nia"),
                diagnostic: nia_diagnostic::Diagnostic::user_error(
                    nia_diagnostic::codes::TYPE_CHECK,
                    "derived wrapper error",
                )
                .finish(),
            },
        ];

        focus_program_diagnostics(&mut diagnostics, &SourcePath::new(BUILD_RUNNER_SOURCE_PATH));

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path.as_str(), "/workspace/build.nia");
        assert!(diagnostics[0].diagnostic.summary.contains("unknown value"));
    }

    #[test]
    fn runner_focus_preserves_dependency_roots_and_attaches_only_proven_context() {
        use nia_diagnostic::{Diagnostic, codes};
        use nia_span::Span;
        let runner = SourcePath::new(BUILD_RUNNER_SOURCE_PATH);
        let dependency = SourcePath::new("/workspace/helpers.nia");
        let span = Span::new(4, 11);
        let source_root = ProgramDiagnostic {
            path: dependency,
            diagnostic: Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                span,
                "unknown helper value",
            ),
        };
        let generated = ProgramDiagnostic {
            path: runner.clone(),
            diagnostic: Diagnostic::user_error(codes::TYPE_CHECK, "derived call failure")
                .primary(Span::new(80, 90), "generated call")
                .caused_by(source_root.path.as_str(), "E0201", span)
                .finish(),
        };
        let warning = ProgramDiagnostic {
            path: SourcePath::new("/workspace/another.nia"),
            diagnostic: Diagnostic::user_warning(codes::UNUSED_IMPORT, "unused import").finish(),
        };
        for with_build_error in [false, true] {
            let mut diagnostics = vec![source_root.clone(), warning.clone(), generated.clone()];
            if with_build_error {
                diagnostics.push(ProgramDiagnostic {
                    path: SourcePath::new("/workspace/build.nia"),
                    diagnostic: Diagnostic::user_error_at(
                        codes::NAME_RESOLUTION,
                        span,
                        "independent build error",
                    ),
                });
            }
            focus_program_diagnostics(&mut diagnostics, &runner);
            assert_eq!(diagnostics.len(), 2 + usize::from(with_build_error));
            assert_eq!(diagnostics[0].diagnostic.related.len(), 1);
            assert_eq!(
                diagnostics[0].diagnostic.related[0].source_path.as_deref(),
                Some(runner.as_str())
            );
            assert_eq!(diagnostics[1], warning);
        }
        let mut generated_only = vec![generated.clone()];
        focus_program_diagnostics(&mut generated_only, &runner);
        assert_eq!(generated_only, [generated]);
    }

    /// A maintained target other than the host, to prove the roles stay apart.
    pub(crate) fn foreign_target() -> nia_target::TargetConfig {
        nia_target::SUPPORTED_TARGETS
            .into_iter()
            .find(|target| Some(*target) != nia_target::TargetConfig::host())
            .expect("the matrix has more than one target")
    }

    pub(crate) fn test_toolchain_layout_for(
        artifact_target: nia_target::TargetConfig,
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

        let canonical_root =
            normalize_host_path(root.canonicalize().expect("canonicalize test root"));
        assert_eq!(plan.package_root, canonical_root);
        assert_eq!(plan.build_script, plan.package_root.join("build.nia"));
        assert!(plan.toolchain.compiler_executable().is_file());
        assert!(plan.toolchain.resource_root().is_dir());
        assert_eq!(plan.build_dir, plan.package_root.join("build").join("out"));
        assert_eq!(
            plan.cache_dir,
            plan.package_root.join("build").join("cache")
        );
        assert_eq!(plan.tmp_dir, plan.package_root.join("build").join("tmp"));
        assert_eq!(
            plan.runner_executable.parent(),
            Some(plan.tmp_dir.as_path())
        );
        assert!(
            plan.runner_executable
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("nia-build-runner-")
        );
        assert_eq!(plan.plan_draft.parent(), Some(plan.tmp_dir.as_path()));
        assert!(
            plan.plan_draft
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("build-plan-")
        );
        assert_eq!(plan.runner_config.parent(), Some(plan.tmp_dir.as_path()));
        assert!(
            plan.runner_config
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("build-runner-")
        );
        assert_eq!(
            plan.plan_path,
            plan.package_root
                .join("build")
                .join("out")
                .join("build-plan.bin")
        );
        assert_eq!(plan.step, BuildStepSelection::Default);
        assert_eq!(plan.optimization, OptimizationMode::O0);
    }

    #[test]
    fn resolves_relative_root_to_absolute_invocation_paths() {
        // Cargo runs unit tests from the crate directory; the fixture must stay
        // relative to it, so it goes under the workspace's ignored target/.
        let relative_root = PathBuf::from("../../target")
            .join("nia-build-tests")
            .join(format!("relative-root-{}", std::process::id()));
        let root = env::current_dir()
            .expect("current test directory")
            .join(&relative_root);
        let child = root.join("src/nested");
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("remove old relative-root fixture");
        }
        std::fs::create_dir_all(&child).expect("create relative child");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let invocation = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout()).with_root(relative_root.join("src/nested")),
        )
        .expect("build invocation");

        assert_eq!(
            invocation.package_root,
            normalize_host_path(root.canonicalize().expect("canonical fixture root"))
        );
        for path in [
            &invocation.package_root,
            &invocation.build_script,
            &invocation.build_dir,
            &invocation.cache_dir,
            &invocation.tmp_dir,
            &invocation.runner_executable,
            &invocation.runner_config,
            &invocation.plan_draft,
            &invocation.plan_path,
        ] {
            assert!(
                path.is_absolute(),
                "path is not absolute: {}",
                path.display()
            );
        }
        std::fs::remove_dir_all(&root).expect("remove relative-root fixture");
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
            &error,
            BuildError::PrepareRunnerConfiguration { error, .. }
                if error.kind() == io::ErrorKind::AlreadyExists
        ));
        crate::report::tests::assert_operational_report(
            &error,
            "E0704",
            &[
                "write runner configuration",
                &invocation.runner_config.to_string_lossy(),
            ],
        );
        assert_eq!(
            std::fs::read(&invocation.runner_config).expect("read preserved configuration"),
            expected
        );
    }

    #[test]
    fn runner_spawn_failure_reports_stage_and_cleans_handoff() {
        let root = temp_root("runner_spawn_failure_reports_stage");
        fs::write(root.join("build.nia"), "").unwrap();
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .unwrap();
        prepare_build_directories(&invocation).unwrap();
        let error =
            run_and_cleanup_build_runner(&invocation, &invocation.runner_executable, "test-runner")
                .unwrap_err();
        assert!(
            matches!(&error, BuildError::RunRunner { stdout, stderr, .. } if stdout.is_empty() && stderr.is_empty())
        );
        crate::report::tests::assert_operational_report(
            &error,
            "E0703",
            &[
                "spawn failed",
                &invocation.runner_executable.to_string_lossy(),
            ],
        );
        assert!(!invocation.runner_config.exists());
        assert!(!invocation.plan_draft.exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_runner_retains_output_context_and_cleans_transients() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("failed_runner_retains_output_context");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_forward_output(false),
        )
        .expect("build invocation");
        prepare_build_directories(&invocation).expect("prepare build directories");
        std::fs::create_dir_all(&invocation.tmp_dir).expect("prepare transient directory");
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

        let error =
            run_and_cleanup_build_runner(&invocation, &invocation.runner_executable, "test-runner")
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

        assert_eq!(runner.path, BUILD_RUNNER_SOURCE_PATH);
        assert!(runner.source.contains("using std::build;"));
        assert!(runner.source.contains("using std::fs;"));
        assert!(runner.source.contains("using std::io;"));
        assert!(runner.source.contains("using std::mem;"));
        assert!(runner.source.contains("using std::string;"));
        assert!(runner.source.contains("using buildScript;"));
        assert!(runner.source.contains("struct ConfigCursor"));
        assert!(runner.source.contains("fun readPath("));
        assert!(runner.source.contains("fun readText("));
        assert!(runner.source.contains("fun readTarget("));
        assert!(runner.source.contains("fun readOptimization("));
        assert!(runner.source.contains("runnerConfigChecksum(payload)"));
        assert!(runner.source.contains(&format!(
            "envelope.u32().reportAndExit(init).? != {}u32",
            RUNNER_CONFIG.protocol_epoch
        )));
        assert!(runner.source.contains(&format!(
            "configLen64 > {}u64",
            runner_config::RUNNER_CONFIG_MAX_BYTES
        )));
        assert!(
            runner
                .source
                .contains(std::str::from_utf8(RUNNER_CONFIG.magic).unwrap())
        );
        assert!(runner.source.contains("init.args().len() != 3"));
        assert!(runner.source.contains("equals(&b\"--config\")"));
        assert!(runner.source.contains("fun reportAndExit("));
        assert!(runner.source.contains("fs::Path::fromUtf8("));
        assert!(runner.source.contains("string::String::fromUtf8("));
        assert!(
            runner
                .source
                .contains("let mut initialization = build::Build::init(")
        );
        assert!(
            runner
                .source
                .contains("initialization.finish().reportAndExit(init).?")
        );
        assert!(runner.source.contains("readPath(&mut config"));
        assert!(runner.source.contains("targets.read(&mut config"));
        assert!(runner.source.contains("targets.views(),"));
        assert!(runner.source.contains("hostTarget,"));
        assert!(runner.source.contains("artifactTarget,"));
        assert!(
            runner
                .source
                .contains("let defaultOptimization = readOptimization(&mut config)")
        );
        assert!(!runner.source.contains("__NIA_BUILD_PROTOCOL_EPOCH__"));
        assert!(!runner.source.contains("__NIA_TARGET_COUNT__"));
        assert!(runner.source.contains("defaultOptimization,"));
        assert!(runner.source.contains(&format!(
            "{}u32,",
            nia_compat::formats::BUILD_PLAN.protocol_epoch
        )));
        assert!(runner.source.contains("        requestedStep,"));
        assert!(!runner.source.contains("pathArg("));
        assert!(!runner.source.contains("stepArgIndex"));
        assert!(runner.source.contains("api.writePlanDraft(planDraft)"));
        assert!(runner.source.contains("api.reportError(error)"));
        assert!(runner.source.contains(").reportAndExit(init).?;"));
        assert!(!runner.source.contains("runRequestedStep"));
        assert!(!runner.source.contains("reportActions"));
        assert!(runner.source.contains("match buildScript::build(&mut api)"));
        assert!(runner.source.contains("return error.intoError()!;"));
        assert!(!runner.source.contains("const"));
    }

    #[cfg(feature = "llvm-static")]
    #[test]
    fn generated_runner_checks_for_maintained_darwin_targets() {
        let root = temp_root("generated_runner_checks_for_maintained_darwin_targets");
        std::fs::write(
            root.join("build.nia"),
            include_str!("../../nia-cli/tests/cases/build/configured_success/build.nia"),
        )
        .expect("write build script");
        for target_name in ["aarch64-apple-macos", "x86_64-apple-macos"] {
            let target = nia_target::TargetConfig::parse(target_name).expect("Darwin target");
            let toolchain = test_toolchain_layout_for(target);
            let plan = resolve_build_invocation(
                BuildRequest::new(Arc::clone(&toolchain)).with_root(&root),
            )
            .expect("build invocation");
            let runner = build_runner_source(&plan).expect("runner source");
            let driver = Driver::with_config(
                DriverConfig::new(Arc::clone(&toolchain)).with_artifact_target(target),
            );
            driver
                .set_source(runner.path.clone(), runner.source.clone())
                .expect("set runner source");
            let module_map = build_runner_module_map(&plan).expect("runner module map");
            let request = CheckRequest::new(runner.path.clone())
                .with_module_map(module_map)
                .with_profile(plan.profile)
                .with_optimization(BUILD_RUNNER_OPTIMIZATION)
                .with_runtime(
                    nia_toolchain::RuntimeSpec::freestanding(&toolchain, &target)
                        .expect("Darwin runtime"),
                );
            driver
                .check_entry(request.clone())
                .result
                .expect("Darwin runner semantic check");
            driver
                .emit_native_objects(EmitObjectRequest::new(request))
                .result
                .expect("Darwin runner backend check");
        }
    }

    #[test]
    fn build_runner_uses_host_o0_and_transports_artifact_configuration() {
        let artifact_target = foreign_target();
        let toolchain = test_toolchain_layout_for(artifact_target);
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
        assert_eq!(
            BUILD_RUNNER_OPTIMIZATION,
            nia_driver::NiaOptimizationLevel::O0
        );
        assert_eq!(config.artifact_target, *toolchain.host_target());
        assert_ne!(config.artifact_target, *toolchain.artifact_target());
        assert_eq!(&encoded[..8], RUNNER_CONFIG.magic);
        assert_eq!(
            u32::from_le_bytes(encoded[8..12].try_into().unwrap()),
            RUNNER_CONFIG.protocol_epoch
        );
        let artifact_name = artifact_target.name();
        assert!(
            encoded
                .windows(artifact_name.len())
                .any(|value| value == artifact_name.as_bytes())
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

    // `nia-test-support` spells the variable itself; keep the names equal.
    #[test]
    fn shared_artifact_cache_override_is_absolute_and_package_caches_stay_local() {
        assert_eq!(
            TEST_SHARED_ARTIFACT_CACHE_ENV,
            "NIA_TEST_SHARED_ARTIFACT_CACHE"
        );
        let root = temp_root("shared_artifact_cache_override");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");
        assert_eq!(
            invocation.cache_dir,
            invocation.package_root.join("build").join("cache")
        );
        match shared_artifact_cache_dir() {
            Some(shared) => assert_eq!(invocation.artifact_cache_dir, shared),
            None => assert_eq!(invocation.artifact_cache_dir, invocation.cache_dir),
        }
    }

    #[test]
    fn listing_overrides_the_step_and_keeps_the_profile() {
        let root = temp_root("listing_overrides_the_step_and_keeps_the_profile");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        for profile in [BuildProfile::Debug, BuildProfile::Release] {
            let invocation = resolve_build_invocation(
                BuildRequest::new(test_toolchain_layout())
                    .with_root(&root)
                    .with_profile(profile)
                    .with_step("test")
                    .with_list(true),
            )
            .expect("listing invocation");

            assert_eq!(invocation.profile, profile);
            assert_eq!(invocation.step, BuildStepSelection::List);
        }
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
    fn a_child_manifest_is_its_own_package_root() {
        let root = temp_root("a_child_manifest_is_its_own_package_root");
        let child = root.join("vendor").join("tool");
        std::fs::create_dir_all(&child).expect("create child package");
        std::fs::write(root.join("build.nia"), "").expect("write parent build script");
        std::fs::write(
            child.join("nia.toml"),
            "[package]
name = \"tool\"
version = \"1.0.0\"
",
        )
        .expect("write child package manifest");

        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&child))
                .expect("child manifest roots its own package");

        assert_eq!(invocation.package_root, child);
        assert!(!invocation.build_script.is_file());
    }

    #[test]
    fn workspace_selection_uses_defaults_members_and_explicit_packages() {
        let root = temp_root("workspace_selection");
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"core\", \"cli\"]\ndefault-members = [\"core\"]\n",
        )
        .expect("write workspace manifest");
        for (directory, name) in [("core", "core"), ("cli", "cli")] {
            let member = root.join(directory);
            std::fs::create_dir_all(&member).expect("create workspace member");
            std::fs::write(
                member.join("nia.toml"),
                format!("[package]\nversion = \"1.0.0\"\nname = \"{name}\"\n"),
            )
            .expect("write member manifest");
            std::fs::write(member.join("build.nia"), "").expect("write build script");
        }

        let select = |request: BuildRequest| {
            select_workspace_members(&request)
                .expect("select workspace members")
                .expect("workspace discovered")
                .into_iter()
                .map(|member| member.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            select(BuildRequest::new(test_toolchain_layout()).with_root(&root)),
            ["core"]
        );
        assert_eq!(
            select(BuildRequest::new(test_toolchain_layout()).with_root(root.join("cli"))),
            ["cli"]
        );
        assert_eq!(
            select(
                BuildRequest::new(test_toolchain_layout())
                    .with_root(&root)
                    .with_package("cli")
            ),
            ["cli"]
        );
        assert_eq!(
            select(
                BuildRequest::new(test_toolchain_layout())
                    .with_root(&root)
                    .with_workspace_all(true)
            ),
            ["cli", "core"]
        );
    }

    #[test]
    fn workspace_selection_flags_require_a_workspace() {
        let root = temp_root("workspace_selection_requires_workspace");
        let error = select_workspace_members(
            &BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_package("core"),
        )
        .expect_err("package selector needs workspace");
        assert!(error.to_string().contains("require a workspace manifest"));
    }

    #[test]
    fn workspace_build_checks_registry_constraints_before_building_members() {
        let root = temp_root("workspace_registry_constraints");
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"left\", \"right\"]\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        for (name, requirement) in [("left", "^1.0"), ("right", "^2.0")] {
            let member = root.join(name);
            std::fs::create_dir_all(&member).unwrap();
            std::fs::write(
                member.join("nia.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n[dependencies]\njson = {{ registry = \"nia\", package = \"acme/json\", version = \"{requirement}\" }}\n"
                ),
            )
            .unwrap();
            std::fs::write(member.join("build.nia"), "").unwrap();
        }

        let cache = nia_package_metadata::RegistryCache::new(root.join("build/registry"));
        let mut index = String::from("schema_version = 1\n");
        for version in ["1.5.0", "2.1.0"] {
            let manifest = format!(
                "[package]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\n"
            );
            let entries = [
                ("nia.toml", manifest.as_bytes()),
                ("src/root.nia", b"pub fun value() -> i32 = 1;".as_slice()),
            ];
            let mut archive = Vec::new();
            archive.extend_from_slice(b"NIAPKG\0\0");
            archive.extend_from_slice(&1u32.to_le_bytes());
            archive.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (path, contents) in entries {
                archive.push(0);
                archive.extend_from_slice(&(path.len() as u32).to_le_bytes());
                archive.extend_from_slice(path.as_bytes());
                archive.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                archive.extend_from_slice(contents);
            }
            let digest_hash = blake3::hash(&archive);
            let digest = *digest_hash.as_bytes();
            cache.write_archive(&digest, &archive).unwrap();
            index.push_str(&format!(
                "[[packages]]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\narchive_url = \"https://cdn.example.test/json-{version}.nia\"\narchive_digest = \"blake3:{}\"\n",
                digest_hash.to_hex()
            ));
        }
        cache.write_index("nia", index.as_bytes()).unwrap();

        let error = run_build(
            BuildRequest::new(test_toolchain_layout())
                .with_root(&root)
                .with_workspace_all(true)
                .with_lock_policy(LockPolicy {
                    locked: false,
                    offline: true,
                }),
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("no single version"), "{message}");
        assert!(message.contains("left@1.0.0 -> json"), "{message}");
        assert!(message.contains("right@1.0.0 -> json"), "{message}");
        assert!(!root.join("build/out/left").exists());
        assert!(!root.join("build/out/right").exists());
        std::fs::remove_dir_all(root).unwrap();
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
        let package_root = root.join("quote-slash tab");
        std::fs::create_dir_all(&package_root).expect("create package root");
        std::fs::write(package_root.join("build.nia"), "").expect("write build script");

        let plan = resolve_build_invocation(
            BuildRequest::new(test_toolchain_layout()).with_root(&package_root),
        )
        .expect("build invocation");
        let runner = build_runner_source(&plan).expect("build runner source");

        assert_eq!(runner.path, BUILD_RUNNER_SOURCE_PATH);
        assert!(
            !runner
                .path
                .contains(&package_root.to_string_lossy().to_string())
        );
        assert!(
            !runner
                .source
                .contains(&package_root.to_string_lossy().to_string())
        );
        assert!(!runner.source.contains("quote\\\"slash"));
    }

    #[test]
    fn own_library_is_bound_by_package_name_and_rejects_a_same_named_dependency() {
        let root = normalize_host_path(
            temp_root("own_library_binding")
                .canonicalize()
                .expect("canonicalize temp root"),
        );
        std::fs::create_dir_all(root.join("src")).expect("create sources");
        std::fs::write(root.join("src/root.nia"), "").expect("write library");
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"hello\"\n[library]\npath = \"src/root.nia\"\n",
        )
        .expect("write manifest");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve build invocation");
        let mut module_map = ModuleMap::new();
        add_manifest_module_roots(
            &mut module_map,
            &invocation.package_graph_report,
            &[DependencyScope::Runtime],
            CompilationMode::Normal,
        )
        .expect("add manifest roots");
        add_own_library_module_root(
            &mut module_map,
            &invocation.package_graph_report,
            &invocation.package_root,
        )
        .expect("bind own library");
        let main = SourcePath::with_identity(
            root.join("src/main.nia").to_string_lossy(),
            "build-package:root:/src/main.nia",
        );
        assert_eq!(
            module_map
                .for_source(&main)
                .get("hello")
                .map(|path| path.identity_ref().normalized_path().to_owned()),
            Some("build-package:root:/src/root.nia".to_owned())
        );

        std::fs::create_dir_all(root.join("vendor/hello")).expect("create dependency");
        std::fs::write(
            root.join("vendor/hello/nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"other\"\n",
        )
        .expect("write dependency manifest");
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"hello\"\n[library]\npath = \"src/root.nia\"\n[dependencies]\nhello = { path = \"vendor/hello\" }\n",
        )
        .expect("rewrite manifest");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve build invocation");
        let error = add_own_library_module_root(
            &mut ModuleMap::new(),
            &invocation.package_graph_report,
            &invocation.package_root,
        )
        .expect_err("dependency shadows the package name");
        assert!(
            error.to_string().contains("same name as the package"),
            "{error}"
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn build_runner_module_map_exposes_declared_build_dependency_libraries() {
        // Package metadata reports canonical roots, which expand 8.3 short
        // names such as a CI runner's `RUNNER~1` temp directory.
        let root = normalize_host_path(
            temp_root("build_runner_build_dependencies")
                .canonicalize()
                .expect("canonicalize temp root"),
        );
        let dependency = root.join("vendor/json");
        std::fs::create_dir_all(&dependency).expect("create build dependency");
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"app\"\n[build-dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .expect("write root manifest");
        std::fs::write(
            dependency.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"json\"\n[library]\npath = \"src/json.nia\"\n",
        )
        .expect("write dependency manifest");
        std::fs::create_dir_all(dependency.join("src")).expect("create library source directory");
        std::fs::write(
            dependency.join("src/json.nia"),
            "pub const value: u8 = 1;\n",
        )
        .expect("write library source");
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve build invocation");
        let module_map = build_runner_module_map(&invocation).expect("runner module map");
        let build_source = SourcePath::with_identity(
            invocation.build_script.to_string_lossy(),
            "build-package:root:/build.nia",
        );
        assert_eq!(
            module_map
                .for_source(&build_source)
                .get("json")
                .map(|path| path.as_str().replace('\\', "/")),
            Some(
                dependency
                    .join("src/json.nia")
                    .to_string_lossy()
                    .replace('\\', "/")
            )
        );
        assert!(module_map.get("json").is_none());
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn build_runner_isolates_transitive_dependency_names() {
        let root = temp_root("build_runner_ambiguous_transitive_dependency_names");
        let first = root.join("first");
        let second = root.join("second");
        let shared_first = root.join("shared-first");
        let shared_second = root.join("shared-second");
        for package in [&first, &second, &shared_first, &shared_second] {
            std::fs::create_dir_all(package.join("src")).expect("create package source");
            std::fs::write(package.join("src/root.nia"), "pub const value: u8 = 1;\n")
                .expect("write package library");
        }
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"app\"\n[build-dependencies]\nfirst = { path = \"first\" }\nsecond = { path = \"second\" }\n",
        )
        .expect("write root manifest");
        for (package, name, shared_path) in [
            (&first, "first", "../shared-first"),
            (&second, "second", "../shared-second"),
        ] {
            std::fs::write(
                package.join("nia.toml"),
                format!(
                    "[package]\nversion = \"1.0.0\"\nname = \"{name}\"\n[library]\npath = \"src/root.nia\"\n[build-dependencies]\nshared = {{ path = \"{shared_path}\" }}\n"
                ),
            )
            .expect("write dependency manifest");
        }
        for (package, name) in [
            (&shared_first, "shared-first"),
            (&shared_second, "shared-second"),
        ] {
            std::fs::write(
                package.join("nia.toml"),
                format!("[package]\nversion = \"1.0.0\"\nname = \"{name}\"\n[library]\npath = \"src/root.nia\"\n"),
            )
            .expect("write transitive manifest");
        }
        std::fs::write(root.join("build.nia"), "").expect("write build script");

        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve build invocation");
        let map = build_runner_module_map(&invocation).expect("build scoped module map");
        let first_scope = SourcePath::with_identity(
            first.join("src/root.nia").to_string_lossy(),
            "build-package:first:/src/root.nia",
        );
        let second_scope = SourcePath::with_identity(
            second.join("src/root.nia").to_string_lossy(),
            "build-package:second:/src/root.nia",
        );
        assert!(map.get("shared").is_none());
        assert!(
            map.for_source(&first_scope)
                .get("shared")
                .unwrap()
                .as_str()
                .ends_with("shared-first/src/root.nia")
        );
        assert!(
            map.for_source(&second_scope)
                .get("shared")
                .unwrap()
                .as_str()
                .ends_with("shared-second/src/root.nia")
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn manifest_dependency_references_enforce_declaration_and_scope() {
        use nia_package_metadata::{DependencyScope, PackageDependency, PackageDependencyReport};

        let root = temp_root("manifest_dependency_reference_validation");
        let runtime = root.join("runtime");
        let build = root.join("build-dependency");
        let development = root.join("development");
        for path in [&runtime, &build, &development] {
            std::fs::create_dir_all(path).expect("create dependency");
        }
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"root\"\n[dependencies]\nruntime = { path = \"runtime\" }\n[build-dependencies]\nbuild = { path = \"build-dependency\" }\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n",
        )
        .expect("write manifest");
        for (path, name) in [
            (&runtime, "runtime"),
            (&build, "build"),
            (&development, "development"),
        ] {
            std::fs::write(
                path.join("nia.toml"),
                format!("[package]\nversion = \"1.0.0\"\nname = \"{name}\"\n"),
            )
            .expect("write dependency manifest");
        }
        let mut invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve invocation");
        invocation.package_graph_report.declared = [
            ("runtime", DependencyScope::Runtime),
            ("build", DependencyScope::Build),
            ("development", DependencyScope::Development),
        ]
        .into_iter()
        .map(|(name, scope)| PackageDependencyReport {
            owner_root: String::new(),
            dependency: PackageDependency {
                name: name.to_string(),
                source: nia_package_metadata::DependencySource::Path(name.to_string()),
                scope,
            },
            resolved_root: Some(name.to_string()),
            source_root: Some(
                root.join(name)
                    .canonicalize()
                    .expect("canonical dependency"),
            ),
        })
        .collect();

        let package = |name: &str| PlanPackage {
            key: PackageKey::new(name).expect("package key"),
            source: PlanPackageSource::Dependency(name.to_string()),
        };
        assert!(
            validate_manifest_dependency_references(&[package("runtime")], &invocation).is_ok()
        );
        assert!(
            validate_manifest_dependency_references(&[package("missing")], &invocation)
                .unwrap_err()
                .contains("not declared")
        );
        assert!(
            validate_manifest_dependency_references(&[package("build")], &invocation)
                .unwrap_err()
                .contains("build-only")
        );
        assert!(
            validate_manifest_dependency_references(&[package("development")], &invocation).is_ok()
        );

        invocation.package_graph_report.declared[2].resolved_root = None;
        assert!(
            validate_manifest_dependency_references(&[package("development")], &invocation)
                .unwrap_err()
                .contains("not present")
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn package_roots_distinguish_manifest_and_local_sources() {
        // See `build_runner_module_map_exposes_declared_build_dependency_libraries`.
        let root = normalize_host_path(
            temp_root("package_roots_manifest_and_local_sources")
                .canonicalize()
                .expect("canonicalize temp root"),
        );
        for path in ["root-bar", "foo", "foo/bar"] {
            std::fs::create_dir_all(root.join(path)).expect("create package root");
        }
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"root\"\n[dependencies]\nbar = { path = \"root-bar\" }\nfoo = { path = \"foo\" }\n",
        )
        .expect("write root manifest");
        std::fs::write(
            root.join("root-bar/nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"bar\"\n",
        )
        .expect("write root bar manifest");
        std::fs::write(
            root.join("foo/nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"foo\"\n[dependencies]\nbar = { path = \"bar\" }\n",
        )
        .expect("write foo manifest");
        std::fs::write(
            root.join("foo/bar/nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"bar\"\n",
        )
        .expect("write transitive bar manifest");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        let invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("resolve package graph");
        let direct = PlanPackage {
            key: PackageKey::new("bar").unwrap(),
            source: PlanPackageSource::Dependency("bar".into()),
        };
        let transitive = PlanPackage {
            key: PackageKey::new("nested-bar").unwrap(),
            source: crate::PlanPackageSource::Local("foo/bar".into()),
        };
        assert_eq!(
            package_source_root(&invocation, &direct),
            Some(root.join("root-bar"))
        );
        assert_eq!(
            package_source_root(&invocation, &transitive),
            Some(root.join("foo/bar"))
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn workspace_ssh_build_dependency_fetches_locks_and_runs_build_script() {
        let _environment = SSH_ENVIRONMENT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let toolchain = test_toolchain_layout();
        nia_git::with_test_ssh_checkout_fixture_mutating_repository(
            |repository| {
                let nested = repository.join("nested");
                std::fs::create_dir_all(nested.join("src"))
                    .expect("create nested build dependency");
                std::fs::write(
                    repository.join("nia.toml"),
                    "[package]\nversion = \"1.0.0\"\nname = \"sshdep\"\n[library]\npath = \"src/root.nia\"\n[build-dependencies]\nnested = { path = \"nested\" }\n",
                )
                .expect("write SSH package manifest");
                std::fs::write(
                    repository.join("src/root.nia"),
                    "using nested;\npub const value: u8 = nested::value;\n",
                )
                .expect("write SSH package library");
                std::fs::write(
                    nested.join("nia.toml"),
                    "[package]\nversion = \"1.0.0\"\nname = \"nested\"\n[library]\npath = \"src/root.nia\"\n",
                )
                .expect("write nested manifest");
                std::fs::write(nested.join("src/root.nia"), "pub const value: u8 = 7;\n")
                    .expect("write nested library");
            },
            move |_repository, source, ssh_config, _repo| {
                let workspace =
                    nia_test_support::test_dir("workspace-ssh-build-dependency-consumer");
                let consumer = workspace.join("app");
                std::fs::create_dir_all(&consumer).expect("create workspace member");
                std::fs::write(workspace.join("nia.toml"), format!(
                    "[workspace]\nmembers = [\"app\"]\n[workspace.dependencies]\nsshdep = {{ git = \"{source}\" }}\n"
                )).expect("write workspace manifest");
                let ssh_home = workspace.join("ssh-home");
                let ssh_directory = ssh_home.join(".ssh");
                std::fs::create_dir_all(&ssh_directory).expect("create SSH home");
                std::fs::copy(
                    ssh_config.join("known_hosts"),
                    ssh_directory.join("known_hosts"),
                )
                .expect("copy known_hosts");
                std::fs::copy(
                    ssh_config.join("id_ed25519"),
                    ssh_directory.join("id_ed25519"),
                )
                .expect("copy SSH identity");
                nia_git::restrict_test_identity_permissions(&ssh_directory.join("id_ed25519"));

                let ssh_environment = SshHomeEnvironment::redirect(&ssh_home);

                std::fs::write(
                consumer.join("nia.toml"),
                "[package]\nversion = \"1.0.0\"\nname = \"consumer\"\n[build-dependencies]\nsshdep.workspace = true\n",
            )
            .expect("write consumer manifest");
                std::fs::write(
                    consumer.join("build.nia"),
                    r#"using std::build;
using std::fs;
using sshdep;

pub fun build(b: &mut build::Build) build::Error!() {
    let value: u8 = sshdep::value;
    let output = b.addGeneratedFileStep(
        &"generated",
        build::BuildPathView::init(&"generated.txt"),
        &b"ssh dependency loaded\n"[..],
    ).?;
    _ = value;
    b.setDefaultStep(output).?;
    !()
}
"#,
                )
                .expect("write consumer build script");
                let result = run_build(BuildRequest::new(toolchain).with_root(&*workspace));
                drop(ssh_environment);
                result.expect("build with SSH dependency");
                assert!(workspace.join("build/out/app/generated.txt").is_file());
                assert!(!consumer.join("nia.lock").exists());
                assert!(!consumer.join("build/packages").exists());
                assert!(workspace.join("build/packages").is_dir());
                let lock = nia_package_metadata::PackageLock::read(&workspace.join("nia.lock"))
                    .expect("read generated lockfile");
                assert!(lock.package.iter().any(|entry| entry.name == "sshdep"));
                let nia_package_metadata::LockedSource::Git {
                    revision,
                    source_fingerprint,
                    ..
                } = &lock
                    .package
                    .iter()
                    .find(|entry| entry.name == "sshdep")
                    .expect("find Git dependency lock entry")
                    .source
                else {
                    panic!("expected Git lock entry");
                };
                assert_eq!(revision.len(), 40);
                assert!(source_fingerprint.starts_with("blake3:"));
            },
        );
    }

    #[test]
    fn ssh_runtime_dependency_and_transitive_imports_are_manifest_driven() {
        let _environment = SSH_ENVIRONMENT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let toolchain = test_toolchain_layout();
        nia_git::with_test_ssh_checkout_fixture_mutating_repository(
            |repository| {
                let nested = repository.join("nested");
                std::fs::create_dir_all(nested.join("src")).expect("create nested dependency");
                std::fs::write(
                    repository.join("nia.toml"),
                    "[package]\nversion = \"1.0.0\"\nname = \"sshdep\"\n[library]\npath = \"src/root.nia\"\n[dependencies]\nnested = { path = \"nested\" }\n",
                )
                .expect("write SSH package manifest");
                std::fs::write(
                    repository.join("src/root.nia"),
                    "using nested;\npub const value: u8 = nested::value;\n",
                )
                .expect("write SSH package library");
                std::fs::write(
                    nested.join("nia.toml"),
                    "[package]\nversion = \"1.0.0\"\nname = \"nested\"\n[library]\npath = \"src/root.nia\"\n",
                )
                .expect("write transitive manifest");
                std::fs::write(nested.join("src/root.nia"), "pub const value: u8 = 7;\n")
                    .expect("write transitive library");
            },
            move |_repository, source, ssh_config, _repo| {
                let consumer = nia_test_support::test_dir("ssh-runtime-dependency-consumer");
                let ssh_home = consumer.join("ssh-home");
                let ssh_directory = ssh_home.join(".ssh");
                std::fs::create_dir_all(&ssh_directory).expect("create SSH home");
                std::fs::copy(
                    ssh_config.join("known_hosts"),
                    ssh_directory.join("known_hosts"),
                )
                .expect("copy known_hosts");
                std::fs::copy(
                    ssh_config.join("id_ed25519"),
                    ssh_directory.join("id_ed25519"),
                )
                .expect("copy SSH identity");
                nia_git::restrict_test_identity_permissions(&ssh_directory.join("id_ed25519"));

                let ssh_environment = SshHomeEnvironment::redirect(&ssh_home);

                std::fs::write(
                    consumer.join("nia.toml"),
                    format!(
                        "[package]\nversion = \"1.0.0\"\nname = \"consumer\"\n[dependencies]\nsshdep = {{ git = \"{source}\" }}\n"
                    ),
                )
                .expect("write consumer manifest");
                std::fs::create_dir_all(consumer.join("src")).expect("create consumer sources");
                std::fs::write(
                    consumer.join("src/main.nia"),
                    "using std::process;\nusing sshdep;\npub fun main(init: process::Init) process::ExitCode!() {\n    _ = init;\n    let value: u8 = sshdep::value;\n    _ = value;\n    !()\n}\n",
                )
                .expect("write consumer source");
                std::fs::write(
                    consumer.join("build.nia"),
                    r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let mainModule = b.addModule(build::ModuleOptions::init(&"main", fs::PathView::init(&"src/main.nia"))).?;
    let app = b.addExecutable(build::ExecutableOptions::init(&"app", mainModule)).?;
    let check = b.addCheckExecutableStep(&"check", app).?;
    b.setDefaultStep(check).?;
    !()
}
"#,
                )
                .expect("write build script");

                let result = run_build(BuildRequest::new(toolchain).with_root(&consumer));
                drop(ssh_environment);
                result.expect("build with SSH runtime dependency");
                let lock = nia_package_metadata::PackageLock::read(&consumer.join("nia.lock"))
                    .expect("read generated lockfile");
                assert!(lock.package.iter().any(|package| package.name == "sshdep"));
            },
        );
    }

    #[test]
    fn build_explanation_distinguishes_dependency_resolution_loading_and_use() {
        use nia_package_metadata::{
            DependencyScope, PackageDependency, PackageDependencyReport, PackageGraphNode,
            PackageGraphReport, PackageManifest,
        };

        let root = temp_root("build_explanation_dependency_usage");
        std::fs::write(root.join("build.nia"), "").expect("write build script");
        for name in ["runtime", "development", "orphan"] {
            std::fs::create_dir_all(root.join(name)).expect("create dependency root");
        }
        let mut invocation =
            resolve_build_invocation(BuildRequest::new(test_toolchain_layout()).with_root(&root))
                .expect("build invocation");

        let dependency = |name: &str, scope: DependencyScope, resolved_root: Option<&str>| {
            PackageDependencyReport {
                owner_root: String::new(),
                dependency: PackageDependency {
                    name: name.to_string(),
                    source: nia_package_metadata::DependencySource::Path(name.to_string()),
                    scope,
                },
                resolved_root: resolved_root.map(str::to_string),
                source_root: Some(
                    root.join(name)
                        .canonicalize()
                        .expect("canonical dependency root"),
                ),
            }
        };
        let graph_node = |name: &str, graph_root: &str| PackageGraphNode {
            manifest: PackageManifest::implicit(name),
            root: graph_root.to_string(),
            source_fingerprint: [0; 32],
        };
        invocation.package_graph_report = PackageGraphReport {
            resolved: vec![
                graph_node("root", ""),
                graph_node("runtime", "runtime"),
                graph_node("development", "development"),
                graph_node("orphan", "orphan"),
            ],
            declared: vec![
                dependency("runtime", DependencyScope::Runtime, Some("runtime")),
                dependency("development", DependencyScope::Development, None),
                dependency("orphan", DependencyScope::Runtime, Some("orphan")),
            ],
        };

        let root_package = PackageKey::root();
        let runtime_package = PackageKey::new("runtime").unwrap();
        let development_package = PackageKey::new("development").unwrap();
        let root_module = ModuleKey::new(root_package.clone(), "main").unwrap();
        let runtime_module = ModuleKey::new(runtime_package.clone(), "lib").unwrap();
        let development_module = ModuleKey::new(development_package.clone(), "lib").unwrap();
        let check_action = ActionKey::new(root_package.clone(), "check").unwrap();
        let check_step = StepKey::new(root_package.clone(), "check").unwrap();
        let target = *invocation.toolchain.host_target();
        let package_path = |package: PackageKey, path: &str| {
            LogicalPath::new(LogicalPathRoot::Package(package), path).unwrap()
        };
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: root_package.clone(),
            packages: vec![
                PlanPackage {
                    key: root_package.clone(),
                    source: crate::PlanPackageSource::Local(String::new()),
                },
                PlanPackage {
                    key: runtime_package.clone(),
                    source: crate::PlanPackageSource::Local("runtime".to_string()),
                },
                PlanPackage {
                    key: development_package.clone(),
                    source: crate::PlanPackageSource::Local("development".to_string()),
                },
            ],
            host_target: target,
            artifact_target: target,
            modules: vec![
                PlanModule {
                    key: root_module.clone(),
                    root_source: package_path(root_package.clone(), "src/main.nia"),
                    optimization: OptimizationMode::O0,
                    imports: vec![
                        ModuleImport {
                            name: "runtime".to_string(),
                            path: package_path(runtime_package.clone(), "src/root.nia"),
                        },
                        ModuleImport {
                            name: "development".to_string(),
                            path: package_path(development_package.clone(), "src/root.nia"),
                        },
                    ],
                },
                PlanModule {
                    key: runtime_module,
                    root_source: package_path(runtime_package, "src/root.nia"),
                    optimization: OptimizationMode::O0,
                    imports: Vec::new(),
                },
                PlanModule {
                    key: development_module,
                    root_source: package_path(development_package, "src/root.nia"),
                    optimization: OptimizationMode::O0,
                    imports: Vec::new(),
                },
            ],
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: check_action.clone(),
                kind: ActionKind::CompilerCheck {
                    module: root_module,
                    target,
                    runtime: Runtime::Freestanding,
                    mode: CompilationMode::Normal,
                },
            }],
            steps: vec![PlanStep {
                key: check_step.clone(),
                action: check_action.clone(),
                dependencies: Vec::new(),
            }],
            default_step: Some(check_step.clone()),
            selected_step: None,
        })
        .expect("freeze build plan");
        let preview = ExecutionReport {
            steps: vec![check_step],
            actions: vec![check_action],
            action_cache: Vec::new(),
        };

        let text =
            render_build_explanation(&plan, &invocation, ConfigureCacheCandidate::None, &preview);
        assert!(text.contains(
            "package graph: declared_dependencies=3, resolved_dependencies=2, selected_packages=4"
        ));
        assert!(text.contains(
            "root::runtime (Runtime) -> runtime [resolved=true, loaded_modules=1, used=true]"
        ));
        assert!(text.contains(
            "root::development (Development) -> filtered [resolved=false, loaded_modules=1, used=false]"
        ));
        assert!(text.contains(
            "root::orphan (Runtime) -> orphan [resolved=true, loaded_modules=0, used=false]"
        ));

        let json: serde_json::Value = serde_json::from_str(&render_build_explanation_json(
            &plan,
            &invocation,
            ConfigureCacheCandidate::None,
            &preview,
        ))
        .expect("parse build explanation JSON");
        let dependencies = json["package_graph"]["declared"]
            .as_array()
            .expect("declared dependency list");
        assert_eq!(json["package_graph"]["resolved_count"], 4);
        assert_eq!(json["package_graph"]["resolved_dependency_count"], 2);
        let dependency = |name: &str| {
            dependencies
                .iter()
                .find(|dependency| dependency["name"] == name)
                .expect("dependency entry")
        };
        assert_eq!(dependency("runtime")["loaded_modules"], 1);
        assert_eq!(dependency("runtime")["used"], true);
        assert_eq!(dependency("development")["resolved"], false);
        assert_eq!(dependency("development")["loaded_modules"], 1);
        assert_eq!(dependency("orphan")["loaded_modules"], 0);
        assert!(
            dependencies
                .iter()
                .all(|dependency| dependency.get("source_root").is_none())
        );

        let plan_json: serde_json::Value =
            serde_json::from_str(&render_build_plan_json(&plan)).expect("parse plan JSON");
        assert_eq!(plan_json["kind"], "build-plan");
        assert_eq!(plan_json["protocol_epoch"], plan.protocol_epoch());
        assert_eq!(
            plan_json["modules"].as_array().unwrap().len(),
            plan.modules().len()
        );
        assert_eq!(
            plan_json["actions"].as_array().unwrap().len(),
            plan.actions().len()
        );
        assert_eq!(
            plan_json["steps"].as_array().unwrap().len(),
            plan.steps().len()
        );
        assert!(plan_json["actions"][0].get("resource_class").is_some());
        assert_eq!(json["plan"]["kind"], "build-plan");
        assert!(json["observations"].get("valid").is_some());
        assert!(json["tools"].as_array().is_some());

        let cache_report = ExecutionReport {
            steps: Vec::new(),
            actions: Vec::new(),
            action_cache: vec![ActionCacheReport {
                action: plan.actions()[0].key.clone(),
                outcome: ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                    ActionCacheInvalidation::Inputs,
                ])),
            }],
        };
        let cache_json: serde_json::Value =
            serde_json::from_str(&render_action_cache_json(&cache_report, &plan))
                .expect("parse action cache JSON");
        assert_eq!(cache_json["mode"], "execution");
        assert_eq!(
            cache_json["entries"][0]["outcome"],
            "Miss(Invalidated([Inputs]))"
        );
        assert_eq!(cache_json["entries"][0]["decision"]["status"], "miss");
        assert_eq!(
            cache_json["entries"][0]["decision"]["reason"],
            "invalidated"
        );
        assert_eq!(
            cache_json["entries"][0]["decision"]["invalidations"][0],
            "inputs"
        );

        std::fs::remove_dir_all(root).expect("remove test package root");
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

// Tests compile Nia in-process on many threads; they run on the allocator
// the `nia` binary uses.
#[cfg(test)]
nia_alloc::global_allocator!();
