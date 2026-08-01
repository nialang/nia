// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, ExecutableCacheRestore, LinkExecutableRequest,
    ModuleMap, NiaOptimizationLevel, Runtime as DriverRuntime, SourcePath,
};
use nia_query::QuerySession;
use nia_target_config::TargetConfig;

use crate::{
    ActionCacheMissReason, ActionCacheOutcome, ActionCacheReport, ActionKey, ActionKind,
    ActionResourceClass, ArtifactKey, BuildInvocation, BuildPlan, CommandArgument, CommandProgram,
    LogicalPath, LogicalPathRoot, ModuleKey, OptimizationMode, OutputRecoveryError, PackageKey,
    PlanAction, PlanArtifact, PlanModule, Runtime, StepKey, TargetSpec,
    action_cache::{
        CompilerCheckCache, CompilerCheckCacheIdentity, CompilerCheckCacheLookup,
        CompilerEmitCache, CompilerEmitCacheIdentity, CompilerEmitCacheLookup, GeneratedFileCache,
        GeneratedFileCacheIdentity, GeneratedFileCacheLookup,
    },
    lock::{ScopedFileLock, output_lock_path},
    output_recovery::{OutputTransactionJournal, recover_interrupted_output_transactions},
    resources::ActionResourceBudget,
};

const EXTERNAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(7 * 60);
const EXTERNAL_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
const EXTERNAL_WAIT_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub steps: Vec<StepKey>,
    pub actions: Vec<ActionKey>,
    pub action_cache: Vec<ActionCacheReport>,
}

#[derive(Debug)]
pub struct TargetMismatch {
    pub role: &'static str,
    pub expected: TargetSpec,
    pub found: TargetSpec,
}

#[derive(Debug)]
pub struct InvalidModuleImport {
    pub action: ActionKey,
    pub module: ModuleKey,
    pub name: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct ExternalCommandError {
    pub action: ActionKey,
    pub program: String,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub failure: ExternalCommandFailure,
}

#[derive(Debug)]
pub enum ExternalCommandFailure {
    Spawn {
        error: io::Error,
    },
    MissingPipe {
        stream: &'static str,
    },
    Wait {
        error: io::Error,
    },
    CaptureThread {
        stream: &'static str,
    },
    CaptureWorkerSpawn {
        stream: &'static str,
        error: io::Error,
    },
    StreamIo {
        stream: &'static str,
        error: io::Error,
    },
    TimedOut {
        timeout: Duration,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Cancelled {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Exit {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

#[derive(Debug)]
pub enum CoordinatorError {
    Cancelled {
        action: ActionKey,
    },
    TargetMismatch(Box<TargetMismatch>),
    InconsistentPlan {
        owner: String,
        missing: String,
    },
    UnmappedPackage {
        action: ActionKey,
        package: PackageKey,
    },
    InvalidModuleImport(Box<InvalidModuleImport>),
    NonUtf8Path {
        action: ActionKey,
        path: PathBuf,
    },
    GeneratedFileIo {
        action: ActionKey,
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
    },
    ExternalCommand(Box<ExternalCommandError>),
    StagedOutput {
        action: ActionKey,
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
        cause: Option<Box<CoordinatorError>>,
    },
    AcquireOutputLock {
        action: ActionKey,
        output: PathBuf,
        lock: PathBuf,
        error: io::Error,
    },
    OutputRecovery(Box<OutputRecoveryError>),
    UnsupportedAction {
        action: ActionKey,
        kind: &'static str,
    },
    Driver {
        action: ActionKey,
        error: Box<DriverError>,
    },
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled { action } => write!(
                f,
                "build action `{}` in package `{}` was cancelled after another action failed",
                action.name(),
                action.package().as_str()
            ),
            Self::TargetMismatch(details) => {
                let TargetMismatch {
                    role,
                    expected,
                    found,
                } = details.as_ref();
                write!(
                    f,
                    "build plan {role} target does not match the invocation: expected {}, found {}",
                    display_target(expected),
                    display_target(found)
                )
            }
            Self::InconsistentPlan { owner, missing } => {
                write!(
                    f,
                    "frozen build plan is inconsistent: {owner} references missing {missing}"
                )
            }
            Self::UnmappedPackage { action, package } => write!(
                f,
                "build action `{}` in package `{}` uses package `{}` without a resolved root",
                action.name(),
                action.package().as_str(),
                package.as_str()
            ),
            Self::InvalidModuleImport(details) => {
                let InvalidModuleImport {
                    action,
                    module,
                    name,
                    reason,
                } = details.as_ref();
                write!(
                    f,
                    "build action `{}` cannot map import `{name}` for module `{}`: {reason}",
                    action.name(),
                    module.name()
                )
            }
            Self::NonUtf8Path { action, path } => write!(
                f,
                "build action `{}` resolved non-UTF-8 path `{}`",
                action.name(),
                path.display()
            ),
            Self::GeneratedFileIo {
                action,
                path,
                operation,
                error,
            } => write!(
                f,
                "build action `{}` failed to {operation} generated file `{}`: {error}",
                action.name(),
                path.display()
            ),
            Self::ExternalCommand(details) => display_external_command_error(f, details),
            Self::StagedOutput {
                action,
                path,
                operation,
                error,
                cause,
            } => {
                write!(
                    f,
                    "build action `{}` failed to {operation} staged output `{}`: {error}",
                    action.name(),
                    path.display()
                )?;
                if let Some(cause) = cause {
                    write!(f, "\noriginal action failure: {cause}")?;
                }
                Ok(())
            }
            Self::AcquireOutputLock {
                action,
                output,
                lock,
                error,
            } => write!(
                f,
                "build action `{}` failed to coordinate publication of `{}` through `{}`: {error}",
                action.name(),
                output.display(),
                lock.display()
            ),
            Self::OutputRecovery(error) => error.fmt(f),
            Self::UnsupportedAction { action, kind } => write!(
                f,
                "build action `{}` uses unsupported coordinator action kind `{kind}`",
                action.name()
            ),
            Self::Driver { action, error } => write!(
                f,
                "compiler action `{}` in package `{}` failed\n{}",
                action.name(),
                action.package().as_str(),
                nia_driver::render_driver_error(error, None, None)
            ),
        }
    }
}

impl std::error::Error for CoordinatorError {}

pub fn execute_build_plan(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<ExecutionReport, CoordinatorError> {
    validate_invocation_targets(plan, invocation)?;
    recover_interrupted_output_transactions(&invocation.cache_dir, &invocation.build_dir)
        .map_err(|error| CoordinatorError::OutputRecovery(Box::new(error)))?;
    let executor = DriverActionExecutor::new(plan.clone(), invocation.clone());
    let session = QuerySession::new();
    nia_timing::emit_counter(
        "build.action_resource_capacity",
        action_resource_capacity(&session, invocation.max_parallel_actions) as u64,
    );
    execute_selected_closure(plan, |actions| {
        let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
        let tasks = actions
            .iter()
            .enumerate()
            .map(|(position, action)| {
                let executor = executor.clone();
                let cancellation = ActionCancellation {
                    earliest_failure: Arc::clone(&earliest_failure),
                    position,
                };
                let action = (*action).clone();
                (action.resource_class(), move || {
                    execute_scheduled_action(&executor, &action, &cancellation)
                })
            })
            .collect::<Vec<_>>();
        run_action_tasks(&session, invocation.max_parallel_actions, tasks)
    })
}

fn run_action_tasks<T, O>(
    session: &QuerySession,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
    tasks: impl IntoIterator<Item = (ActionResourceClass, T)>,
) -> Vec<O>
where
    T: FnOnce() -> O + Send + 'static,
    O: Send + 'static,
{
    let capacity = action_resource_capacity(session, max_parallel_actions);
    let budget = Arc::new(ActionResourceBudget::new(capacity));
    let tasks = tasks
        .into_iter()
        .map(|(resource_class, task)| {
            let budget = Arc::clone(&budget);
            move || {
                let _permit = budget.acquire(resource_class);
                nia_timing::emit_counter(resource_class_counter(resource_class), 1);
                task()
            }
        })
        .collect::<Vec<_>>();
    match max_parallel_actions {
        Some(limit) => session.run_tasks_bounded(tasks, limit.get()),
        None => session.run_tasks(tasks),
    }
}

fn action_resource_capacity(
    session: &QuerySession,
    max_parallel_actions: Option<std::num::NonZeroUsize>,
) -> usize {
    max_parallel_actions
        .map_or_else(|| session.executor_parallelism(), |limit| limit.get())
        .min(session.executor_parallelism())
        .max(1)
}

fn resource_class_counter(class: ActionResourceClass) -> &'static str {
    match class {
        ActionResourceClass::Conservative => "build.resource_class_conservative_actions",
        ActionResourceClass::Cpu => "build.resource_class_cpu_actions",
        ActionResourceClass::Io => "build.resource_class_io_actions",
    }
}

enum ActionOutcome {
    Succeeded(Option<ActionCacheOutcome>),
    Cancelled,
    Failed(CoordinatorError),
}

struct ActionCancellation {
    earliest_failure: Arc<AtomicUsize>,
    position: usize,
}

impl ActionCancellation {
    fn is_cancelled(&self) -> bool {
        self.earliest_failure.load(Ordering::Acquire) < self.position
    }

    fn cancel_later_actions(&self) {
        self.earliest_failure
            .fetch_min(self.position, Ordering::AcqRel);
    }
}

fn execute_scheduled_action(
    executor: &DriverActionExecutor,
    action: &PlanAction,
    cancellation: &ActionCancellation,
) -> ActionOutcome {
    if cancellation.is_cancelled() {
        return ActionOutcome::Cancelled;
    }
    match executor.execute(action, cancellation) {
        Ok(cache) => ActionOutcome::Succeeded(cache),
        Err(error) if is_cancellation_error(&error) => ActionOutcome::Cancelled,
        Err(error) => {
            cancellation.cancel_later_actions();
            ActionOutcome::Failed(error)
        }
    }
}

fn execute_selected_closure(
    plan: &BuildPlan,
    mut execute_batch: impl FnMut(&[&PlanAction]) -> Vec<ActionOutcome>,
) -> Result<ExecutionReport, CoordinatorError> {
    let Some(selected) = plan.selected_step().or_else(|| plan.default_step()) else {
        return Ok(ExecutionReport {
            steps: Vec::new(),
            actions: Vec::new(),
            action_cache: Vec::new(),
        });
    };

    let steps = plan.steps();
    let mut closure = BTreeSet::new();
    let selected_index = find_step(steps, selected)
        .ok_or_else(|| inconsistent("plan selection", format!("step `{}`", selected.name())))?;
    let mut pending = vec![selected_index];
    while let Some(index) = pending.pop() {
        if !closure.insert(index) {
            continue;
        }
        for dependency in &steps[index].dependencies {
            pending.push(find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?);
        }
    }

    let mut indegree = vec![0usize; steps.len()];
    let mut dependents = vec![Vec::new(); steps.len()];
    for &index in &closure {
        for dependency in &steps[index].dependencies {
            let dependency_index = find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?;
            if closure.contains(&dependency_index) {
                indegree[index] += 1;
                dependents[dependency_index].push(index);
            }
        }
    }

    let mut ready: BTreeSet<_> = closure
        .iter()
        .copied()
        .filter(|index| indegree[*index] == 0)
        .collect();
    let mut executed_actions = BTreeSet::new();
    let mut report = ExecutionReport {
        steps: Vec::with_capacity(closure.len()),
        actions: Vec::new(),
        action_cache: Vec::new(),
    };

    while !ready.is_empty() {
        let wave = std::mem::take(&mut ready);
        let mut wave_actions = Vec::new();
        for &index in &wave {
            let step = &steps[index];
            if executed_actions.insert(step.action.clone()) {
                let action = find_action(plan.actions(), &step.action).ok_or_else(|| {
                    inconsistent(
                        format!("step `{}`", step.key.name()),
                        format!("action `{}`", step.action.name()),
                    )
                })?;
                wave_actions.push(action);
            }
        }
        let outcomes = execute_batch(&wave_actions);
        if outcomes.len() != wave_actions.len() {
            return Err(inconsistent(
                "coordinator action batch",
                "one outcome per scheduled action".to_string(),
            ));
        }
        let mut cancelled = false;
        for (action, outcome) in wave_actions.iter().zip(outcomes) {
            match outcome {
                ActionOutcome::Succeeded(cache) => {
                    if let Some(outcome) = cache {
                        report.action_cache.push(ActionCacheReport {
                            action: action.key.clone(),
                            outcome,
                        });
                    }
                }
                ActionOutcome::Cancelled => cancelled = true,
                ActionOutcome::Failed(error) => return Err(error),
            }
        }
        if cancelled {
            return Err(inconsistent(
                "coordinator action batch",
                "failure cause for cancelled actions".to_string(),
            ));
        }
        report
            .actions
            .extend(wave_actions.iter().map(|action| action.key.clone()));
        for index in wave {
            let step = &steps[index];
            report.steps.push(step.key.clone());
            for &dependent in &dependents[index] {
                indegree[dependent] = indegree[dependent].checked_sub(1).ok_or_else(|| {
                    inconsistent(
                        format!("step `{}`", steps[dependent].key.name()),
                        "valid dependency degree".to_string(),
                    )
                })?;
                if indegree[dependent] == 0 {
                    ready.insert(dependent);
                }
            }
        }
    }

    if report.steps.len() != closure.len() {
        return Err(inconsistent(
            "selected step closure",
            "acyclic dependency order".to_string(),
        ));
    }
    Ok(report)
}

#[derive(Clone)]
struct DriverActionExecutor {
    plan: Arc<BuildPlan>,
    invocation: Arc<BuildInvocation>,
    drivers: Arc<BTreeMap<TargetSpec, Arc<Driver>>>,
}

impl DriverActionExecutor {
    fn new(plan: BuildPlan, invocation: BuildInvocation) -> Self {
        let plan = Arc::new(plan);
        let invocation = Arc::new(invocation);
        let mut drivers = BTreeMap::new();
        for action in plan.actions() {
            let target = match &action.kind {
                ActionKind::CompilerCheck { target, .. }
                | ActionKind::CompilerEmit { target, .. } => target,
                _ => continue,
            };
            drivers.entry(target.clone()).or_insert_with(|| {
                Arc::new(Driver::with_config(
                    DriverConfig {
                        artifact_cache_dir: Some(invocation.cache_dir.clone()),
                        ..DriverConfig::new(Arc::clone(&invocation.toolchain))
                    }
                    .with_artifact_target(target_config(target)),
                ))
            });
        }
        Self {
            plan,
            invocation,
            drivers: Arc::new(drivers),
        }
    }

    fn execute(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let Some(_output_locks) = self.acquire_output_locks(action, cancellation)? else {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        };
        if cancellation.is_cancelled() {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        }
        self.execute_with_output_ownership(action, cancellation)
    }

    fn execute_with_output_ownership(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let result = match &action.kind {
            ActionKind::Aggregate => Ok(()),
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
            } => {
                return self.execute_compiler_check(action, module, target, *runtime);
            }
            ActionKind::CompilerEmit { artifact, target } => {
                return self.execute_compiler_emit(action, artifact, target);
            }
            ActionKind::ExternalCommand {
                resource_class: _,
                environment_policy,
                cache_policy: _,
                program,
                arguments,
                working_directory,
                environment,
                inputs,
                outputs,
            } => {
                let working_directory = self.resolve_path(action, working_directory)?;
                let program = match program {
                    CommandProgram::Path(path) => {
                        let path = self.resolve_path(action, path)?;
                        path.to_str()
                            .ok_or_else(|| CoordinatorError::NonUtf8Path {
                                action: action.key.clone(),
                                path: path.clone(),
                            })?
                            .to_string()
                    }
                    CommandProgram::Search(name) => name.clone(),
                };
                let resolved_inputs = inputs
                    .iter()
                    .map(|input| self.resolve_path(action, input).map(|path| (input, path)))
                    .collect::<Result<Vec<_>, _>>()?;
                let resolved_outputs = outputs
                    .iter()
                    .map(|output| self.resolve_path(action, output).map(|path| (output, path)))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut staged = if resolved_outputs.is_empty() {
                    None
                } else {
                    Some(prepare_staged_outputs(
                        action,
                        &self.invocation.build_dir,
                        &resolved_outputs,
                    )?)
                };
                let resolved_arguments = arguments
                    .iter()
                    .map(|argument| match argument {
                        CommandArgument::Literal(value) => Ok(value.clone()),
                        CommandArgument::InputPath(path) => {
                            let (_, resolved) = resolved_inputs
                                .iter()
                                .find(|(input, _)| *input == path)
                                .ok_or_else(|| {
                                    inconsistent(
                                        format!("action `{}`", action.key.name()),
                                        "declared command input binding".to_string(),
                                    )
                                })?;
                            path_text(action, resolved)
                        }
                        CommandArgument::OutputPath(path) => {
                            let Some(index) = resolved_outputs
                                .iter()
                                .position(|(output, _)| *output == path)
                            else {
                                return Err(inconsistent(
                                    format!("action `{}`", action.key.name()),
                                    "matching command output binding".to_string(),
                                ));
                            };
                            let staged = staged.as_ref().ok_or_else(|| {
                                inconsistent(
                                    format!("action `{}`", action.key.name()),
                                    "declared command output transaction".to_string(),
                                )
                            })?;
                            path_text(action, &staged.outputs[index].temporary)
                        }
                    })
                    .collect::<Result<Vec<_>, CoordinatorError>>();
                let resolved_arguments = match resolved_arguments {
                    Ok(arguments) => arguments,
                    Err(cause) => {
                        return match staged.take() {
                            Some(staged) => {
                                cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                            }
                            None => Err(cause),
                        }
                        .map(|()| None);
                    }
                };
                let execution = execute_external_command(
                    action,
                    ResolvedExternalCommand {
                        program: &program,
                        arguments: &resolved_arguments,
                        working_directory: &working_directory,
                        environment_policy: *environment_policy,
                        environment,
                    },
                    ExternalExecutionPolicy {
                        timeout: EXTERNAL_COMMAND_TIMEOUT,
                        forward_output: true,
                        cancellation: Some(cancellation),
                    },
                );
                match (execution, staged) {
                    (Ok(()), Some(staged)) if cancellation.is_cancelled() => {
                        let cause =
                            CoordinatorError::ExternalCommand(Box::new(ExternalCommandError {
                                action: action.key.clone(),
                                program,
                                arguments: resolved_arguments,
                                working_directory,
                                failure: ExternalCommandFailure::Cancelled {
                                    stdout: Vec::new(),
                                    stderr: Vec::new(),
                                },
                            }));
                        cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                    }
                    (Ok(()), Some(staged)) => publish_staged_outputs(action, staged),
                    (Ok(()), None) => Ok(()),
                    (Err(cause), Some(staged)) => {
                        cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                    }
                    (Err(cause), None) => Err(cause),
                }
            }
            ActionKind::GeneratedFile { output, contents } => {
                return self.execute_generated_file(action, output, contents);
            }
            ActionKind::Uncacheable { .. } => Err(unsupported(action, "uncacheable")),
        };
        result.map(|()| None)
    }

    fn execute_compiler_check(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
        target: &TargetSpec,
        runtime: Runtime,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let request = self.check_request(action, module_key, runtime)?;
        let driver = self.driver(action, target)?;
        let precheck_manifest = driver
            .source_input_manifest(&request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        let cache = CompilerCheckCache::new(self.invocation.cache_dir.clone());
        let precheck_identity = CompilerCheckCacheIdentity::new(
            &action.key,
            module,
            target,
            runtime,
            &precheck_manifest,
            self.invocation.toolchain.identity(),
        );
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerCheckCacheLookup::Hit) => {
                    return Ok(Some(ActionCacheOutcome::Hit));
                }
                Ok(CompilerCheckCacheLookup::Miss(reason)) => reason,
                Err(_) => ActionCacheMissReason::ReadError,
            },
        };
        let checked = driver
            .check_entry_with_source_manifest(request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        if !checked.program.diagnostics.is_empty() {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let Some(final_identity) = CompilerCheckCacheIdentity::new(
            &action.key,
            module,
            target,
            runtime,
            &checked.source_manifest,
            self.invocation.toolchain.identity(),
        ) else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let reason = match cache.publish(&final_identity) {
            Ok(()) => miss_reason,
            Err(_) => ActionCacheMissReason::WriteError,
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn execute_compiler_emit(
        &self,
        action: &PlanAction,
        artifact_key: &ArtifactKey,
        target: &TargetSpec,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let artifact = self.artifact(action, artifact_key)?;
        let module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request = self
            .check_request(action, &artifact.root_module, artifact.runtime)?
            .with_runtime(DriverRuntime::Freestanding);
        let output = self.resolve_path(action, &artifact.output)?;
        let driver = self.driver(action, target)?;
        let precheck_manifest = driver
            .source_input_manifest(&request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        let link_environment = driver.executable_cache_environment();
        let cache = CompilerEmitCache::new(self.invocation.cache_dir.clone());
        let precheck_identity = link_environment.and_then(|environment| {
            CompilerEmitCacheIdentity::new(
                &action.key,
                artifact,
                module,
                target,
                &precheck_manifest,
                self.invocation.toolchain.identity(),
                environment,
            )
        });
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerEmitCacheLookup::Hit(reference)) => {
                    let reason = match driver.restore_executable_cache(reference, &output) {
                        ExecutableCacheRestore::Hit => {
                            return Ok(Some(ActionCacheOutcome::Hit));
                        }
                        ExecutableCacheRestore::NotFound => ActionCacheMissReason::NotFound,
                        ExecutableCacheRestore::Invalidated => {
                            ActionCacheMissReason::Invalidated(vec![
                                crate::ActionCacheInvalidation::Linker,
                            ])
                        }
                        ExecutableCacheRestore::Corrupt => ActionCacheMissReason::Corrupt,
                        ExecutableCacheRestore::ReadError => ActionCacheMissReason::ReadError,
                        ExecutableCacheRestore::Disabled => ActionCacheMissReason::Uncacheable,
                    };
                    if cache.retire(identity, reference).is_err() {
                        ActionCacheMissReason::ReadError
                    } else {
                        reason
                    }
                }
                Ok(CompilerEmitCacheLookup::Miss(reason)) => reason,
                Err(_) => ActionCacheMissReason::ReadError,
            },
        };
        let linked = driver
            .link_executable_with_source_manifest(LinkExecutableRequest::new(request, output))
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        if !linked.artifact.diagnostics.is_empty() {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let Some(reference) = linked.artifact.cache_reference else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let Some(link_environment) = driver.executable_cache_environment() else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let Some(final_identity) = CompilerEmitCacheIdentity::new(
            &action.key,
            artifact,
            module,
            target,
            &linked.source_manifest,
            self.invocation.toolchain.identity(),
            link_environment,
        ) else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let reason = match cache.publish(&final_identity, reference) {
            Ok(()) => miss_reason,
            Err(_) => ActionCacheMissReason::WriteError,
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn execute_generated_file(
        &self,
        action: &PlanAction,
        logical_output: &LogicalPath,
        contents: &[u8],
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let output = self.resolve_path(action, logical_output)?;
        let cache = GeneratedFileCache::new(self.invocation.cache_dir.clone());
        let identity = GeneratedFileCacheIdentity::new(
            &action.key,
            logical_output,
            contents,
            self.invocation.toolchain.identity(),
        );
        let lookup = match cache.lookup(&identity) {
            Ok(lookup) => lookup,
            Err(_) => {
                write_generated_file(action, &output, contents)?;
                return Ok(Some(ActionCacheOutcome::Miss(
                    ActionCacheMissReason::ReadError,
                )));
            }
        };
        match lookup {
            GeneratedFileCacheLookup::Hit(payload) => {
                write_generated_file(action, &output, &payload)?;
                Ok(Some(ActionCacheOutcome::Hit))
            }
            GeneratedFileCacheLookup::Miss(reason) => {
                write_generated_file(action, &output, contents)?;
                let reason = match cache.publish(&identity, contents) {
                    Ok(()) => reason,
                    Err(_) => ActionCacheMissReason::WriteError,
                };
                Ok(Some(ActionCacheOutcome::Miss(reason)))
            }
        }
    }

    fn acquire_output_locks(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<Vec<ScopedFileLock>>, CoordinatorError> {
        let mut outputs: Vec<&LogicalPath> = match &action.kind {
            ActionKind::CompilerEmit { artifact, .. } => {
                vec![&self.artifact(action, artifact)?.output]
            }
            ActionKind::ExternalCommand { outputs, .. } => outputs.iter().collect(),
            ActionKind::GeneratedFile { output, .. } => vec![output],
            _ => Vec::new(),
        };
        outputs.sort();
        outputs.dedup();
        let mut acquired = Vec::with_capacity(outputs.len());
        for output in outputs {
            let resolved = self.resolve_path(action, output)?;
            let lock = output_lock_path(&self.invocation.cache_dir, output);
            let Some(output_lock) =
                ScopedFileLock::acquire_interruptible(lock.clone(), || cancellation.is_cancelled())
                    .map_err(|error| CoordinatorError::AcquireOutputLock {
                        action: action.key.clone(),
                        output: resolved,
                        lock,
                        error,
                    })?
            else {
                return Ok(None);
            };
            acquired.push(output_lock);
        }
        Ok(Some(acquired))
    }

    fn check_request(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
        runtime: Runtime,
    ) -> Result<CheckRequest, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let entry = self.resolve_source_path(action, &module.root_source)?;
        let mut module_map = ModuleMap::new();
        for import in &module.imports {
            let path = self.resolve_source_path(action, &import.path)?;
            module_map
                .try_insert(&import.name, path)
                .map_err(|reason| {
                    CoordinatorError::InvalidModuleImport(Box::new(InvalidModuleImport {
                        action: action.key.clone(),
                        module: module.key.clone(),
                        name: import.name.clone(),
                        reason,
                    }))
                })?;
        }
        Ok(CheckRequest::from_source_path(entry)
            .with_module_map(module_map)
            .with_optimization(optimization(module.optimization))
            .with_timings(self.invocation.timings)
            .with_runtime(runtime_mode(runtime)))
    }

    fn artifact(
        &self,
        action: &PlanAction,
        key: &ArtifactKey,
    ) -> Result<&PlanArtifact, CoordinatorError> {
        find_artifact(self.plan.artifacts(), key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("artifact `{}`", key.name()),
            )
        })
    }

    fn driver(
        &self,
        action: &PlanAction,
        target: &TargetSpec,
    ) -> Result<&Driver, CoordinatorError> {
        self.drivers.get(target).map(Arc::as_ref).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("compiler driver for target `{}`", display_target(target)),
            )
        })
    }

    fn resolve_source_path(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<SourcePath, CoordinatorError> {
        let path = self.resolve_path(action, logical)?;
        let text = path.to_str().ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.clone(),
        })?;
        let protocol_path = logical.protocol_path();
        let identity = match logical.root() {
            LogicalPathRoot::Package(package) => {
                format!("build-package:{}:/{protocol_path}", package.as_str())
            }
            LogicalPathRoot::Build => format!(
                "build-output:{}:/{protocol_path}",
                self.plan.root_package().as_str()
            ),
            LogicalPathRoot::Cache => format!(
                "build-cache:{}:/{protocol_path}",
                self.plan.root_package().as_str()
            ),
            LogicalPathRoot::Toolchain => format!("toolchain:/{protocol_path}"),
            LogicalPathRoot::Artifact(artifact) => format!(
                "build-artifact:{}:{}:/{protocol_path}",
                artifact.package().as_str(),
                artifact.name()
            ),
        };
        Ok(SourcePath::with_identity(text, identity))
    }

    fn resolve_path(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<PathBuf, CoordinatorError> {
        let mut path = match logical.root() {
            LogicalPathRoot::Package(package) => {
                if package != self.plan.root_package() {
                    return Err(CoordinatorError::UnmappedPackage {
                        action: action.key.clone(),
                        package: package.clone(),
                    });
                }
                self.invocation.package_root.clone()
            }
            LogicalPathRoot::Build => self.invocation.build_dir.clone(),
            LogicalPathRoot::Cache => self.invocation.cache_dir.clone(),
            LogicalPathRoot::Toolchain => self.invocation.toolchain.resource_root().to_path_buf(),
            LogicalPathRoot::Artifact(key) => {
                let artifact = self.artifact(action, key)?;
                self.resolve_path(action, &artifact.output)?
            }
        };
        for component in logical.components() {
            path.push(component);
        }
        Ok(path)
    }
}

struct ResolvedExternalCommand<'a> {
    program: &'a str,
    arguments: &'a [String],
    working_directory: &'a Path,
    environment_policy: crate::CommandEnvironmentPolicy,
    environment: &'a [crate::EnvironmentInput],
}

#[derive(Clone, Copy)]
struct ExternalExecutionPolicy<'a> {
    timeout: Duration,
    forward_output: bool,
    cancellation: Option<&'a ActionCancellation>,
}

fn execute_external_command(
    action: &PlanAction,
    request: ResolvedExternalCommand<'_>,
    policy: ExternalExecutionPolicy,
) -> Result<(), CoordinatorError> {
    let error = |failure| {
        CoordinatorError::ExternalCommand(Box::new(ExternalCommandError {
            action: action.key.clone(),
            program: request.program.to_string(),
            arguments: request.arguments.to_vec(),
            working_directory: request.working_directory.to_path_buf(),
            failure,
        }))
    };
    let mut command = Command::new(request.program);
    command
        .args(request.arguments)
        .current_dir(request.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.environment_policy == crate::CommandEnvironmentPolicy::Clear {
        command.env_clear();
    }
    for input in request.environment {
        match &input.value {
            Some(value) => {
                command.env(&input.name, value);
            }
            None => {
                command.env_remove(&input.name);
            }
        }
    }
    prepare_external_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|source| error(ExternalCommandFailure::Spawn { error: source }))?;
    let Some(stdout) = child.stdout.take() else {
        terminate_process_tree(&mut child);
        let _ = child.wait();
        return Err(error(ExternalCommandFailure::MissingPipe {
            stream: "stdout",
        }));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_process_tree(&mut child);
        let _ = child.wait();
        return Err(error(ExternalCommandFailure::MissingPipe {
            stream: "stderr",
        }));
    };
    let stdout_reader = match thread::Builder::new()
        .name("nia-build-stdout".to_string())
        .spawn(move || capture_stream(stdout, CapturedStream::Stdout, policy.forward_output))
    {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stdout",
                error: source,
            }));
        }
    };
    let stderr_reader = match thread::Builder::new()
        .name("nia-build-stderr".to_string())
        .spawn(move || capture_stream(stderr, CapturedStream::Stderr, policy.forward_output))
    {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let _ = stdout_reader.join();
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stderr",
                error: source,
            }));
        }
    };

    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if policy
            .cancellation
            .is_some_and(ActionCancellation::is_cancelled)
        {
            cancelled = true;
            terminate_process_tree(&mut child);
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_external_descendants(child.id());
                break Ok(status);
            }
            Ok(None) if started.elapsed() >= policy.timeout => {
                timed_out = true;
                terminate_process_tree(&mut child);
                break child.wait();
            }
            Ok(None) => thread::sleep(EXTERNAL_WAIT_POLL),
            Err(source) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                break Err(source);
            }
        }
    };
    let stdout = join_capture(stdout_reader, "stdout").map_err(&error)?;
    let stderr = join_capture(stderr_reader, "stderr").map_err(&error)?;
    if let Some(source) = stdout.error {
        return Err(error(ExternalCommandFailure::StreamIo {
            stream: "stdout",
            error: source,
        }));
    }
    if let Some(source) = stderr.error {
        return Err(error(ExternalCommandFailure::StreamIo {
            stream: "stderr",
            error: source,
        }));
    }
    let status = status.map_err(|source| error(ExternalCommandFailure::Wait { error: source }))?;
    if cancelled {
        return Err(error(ExternalCommandFailure::Cancelled {
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    if timed_out {
        return Err(error(ExternalCommandFailure::TimedOut {
            timeout: policy.timeout,
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    if !status.success() {
        return Err(error(ExternalCommandFailure::Exit {
            status,
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CapturedStream {
    Stdout,
    Stderr,
}

struct StreamCapture {
    tail: Vec<u8>,
    error: Option<io::Error>,
}

fn capture_stream(
    mut reader: impl io::Read,
    stream: CapturedStream,
    forward_output: bool,
) -> StreamCapture {
    let mut tail = Vec::new();
    let mut first_error = None;
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                append_output_tail(&mut tail, &buffer[..count]);
                let forwarded = match (forward_output, stream) {
                    (false, _) => Ok(()),
                    (true, CapturedStream::Stdout) => io::stdout().write_all(&buffer[..count]),
                    (true, CapturedStream::Stderr) => io::stderr().write_all(&buffer[..count]),
                };
                if first_error.is_none() {
                    first_error = forwarded.err();
                }
            }
            Err(source) => {
                if first_error.is_none() {
                    first_error = Some(source);
                }
                break;
            }
        }
    }
    if forward_output {
        let flushed = match stream {
            CapturedStream::Stdout => io::stdout().flush(),
            CapturedStream::Stderr => io::stderr().flush(),
        };
        if first_error.is_none() {
            first_error = flushed.err();
        }
    }
    StreamCapture {
        tail,
        error: first_error,
    }
}

fn append_output_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= EXTERNAL_OUTPUT_TAIL_BYTES {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - EXTERNAL_OUTPUT_TAIL_BYTES..]);
        return;
    }
    let excess = tail
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(EXTERNAL_OUTPUT_TAIL_BYTES);
    if excess != 0 {
        tail.drain(..excess);
    }
    tail.extend_from_slice(bytes);
}

fn join_capture(
    reader: thread::JoinHandle<StreamCapture>,
    stream: &'static str,
) -> Result<StreamCapture, ExternalCommandFailure> {
    reader
        .join()
        .map_err(|_| ExternalCommandFailure::CaptureThread { stream })
}

#[cfg(unix)]
fn prepare_external_command(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn prepare_external_command(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_tree(child: &mut std::process::Child) {
    let Ok(group) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    terminate_process_group(group);
}

#[cfg(unix)]
fn terminate_external_descendants(group: u32) {
    if let Ok(group) = i32::try_from(group) {
        terminate_process_group(group);
    }
}

#[cfg(unix)]
fn terminate_process_group(group: i32) {
    // The child is the leader of the process group created before spawn. A
    // successful signal means at least one owned process still needs cleanup.
    let signaled = unsafe { libc::kill(-group, libc::SIGTERM) } == 0;
    if signaled {
        thread::sleep(Duration::from_millis(100));
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate_external_descendants(_group: u32) {}

fn display_external_command_error(
    f: &mut fmt::Formatter<'_>,
    details: &ExternalCommandError,
) -> fmt::Result {
    write!(
        f,
        "external command action `{}` in package `{}` failed to run `{:?}` with {} argument(s) in `{}`: ",
        details.action.name(),
        details.action.package().as_str(),
        details.program,
        details.arguments.len(),
        details.working_directory.display(),
    )?;
    match &details.failure {
        ExternalCommandFailure::Spawn { error } => write!(f, "spawn failed: {error}"),
        ExternalCommandFailure::MissingPipe { stream } => {
            write!(f, "coordinator did not retain the configured {stream} pipe")
        }
        ExternalCommandFailure::Wait { error } => write!(f, "wait failed: {error}"),
        ExternalCommandFailure::CaptureThread { stream } => {
            write!(f, "{stream} capture worker failed")
        }
        ExternalCommandFailure::CaptureWorkerSpawn { stream, error } => {
            write!(f, "failed to start {stream} capture worker: {error}")
        }
        ExternalCommandFailure::StreamIo { stream, error } => {
            write!(f, "{stream} capture/forward failed: {error}")
        }
        ExternalCommandFailure::TimedOut {
            timeout,
            stdout,
            stderr,
        } => {
            write!(f, "timed out after {timeout:?}")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::Cancelled { stdout, stderr } => {
            f.write_str("cancelled after another build action failed")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::Exit {
            status,
            stdout,
            stderr,
        } => {
            write!(f, "exited with status {status}")?;
            display_output_tails(f, stdout, stderr)
        }
    }
}

fn is_cancellation_error(error: &CoordinatorError) -> bool {
    match error {
        CoordinatorError::Cancelled { .. } => true,
        CoordinatorError::ExternalCommand(details) => {
            matches!(details.failure, ExternalCommandFailure::Cancelled { .. })
        }
        CoordinatorError::StagedOutput {
            cause: Some(cause), ..
        } => is_cancellation_error(cause),
        _ => false,
    }
}

fn display_output_tails(f: &mut fmt::Formatter<'_>, stdout: &[u8], stderr: &[u8]) -> fmt::Result {
    if !stdout.is_empty() {
        write!(f, "\nstdout tail:\n{}", String::from_utf8_lossy(stdout))?;
    }
    if !stderr.is_empty() {
        write!(f, "\nstderr tail:\n{}", String::from_utf8_lossy(stderr))?;
    }
    Ok(())
}

fn path_text(action: &PlanAction, path: &Path) -> Result<String, CoordinatorError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.to_path_buf(),
        })
}

static STAGED_OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct StagedOutputTransaction {
    directory: PathBuf,
    committed_directory: PathBuf,
    outputs: Vec<StagedOutputEntry>,
    journal: OutputTransactionJournal,
}

struct StagedOutputEntry {
    destination: PathBuf,
    temporary: PathBuf,
    backup: PathBuf,
}

struct StagedOutputPublication {
    had_previous: bool,
    backed_up: bool,
    installed: bool,
}

fn prepare_staged_outputs(
    action: &PlanAction,
    build_dir: &Path,
    resolved_outputs: &[(&LogicalPath, PathBuf)],
) -> Result<StagedOutputTransaction, CoordinatorError> {
    let (_, first) = resolved_outputs.first().ok_or_else(|| {
        staged_output_io(
            action,
            Path::new(""),
            "resolve transaction root for",
            io::Error::new(io::ErrorKind::InvalidInput, "output transaction is empty"),
            None,
        )
    })?;
    let parent = first
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            staged_output_io(
                action,
                first,
                "resolve parent for",
                io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"),
                None,
            )
        })?;
    fs::create_dir_all(parent).map_err(|error| {
        staged_output_io(action, parent, "create parent directory for", error, None)
    })?;
    for _ in 0..128 {
        let sequence = STAGED_OUTPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = parent.join(format!(
            ".nia-command-{}-{sequence}.stage",
            std::process::id()
        ));
        let committed_directory = parent.join(format!(
            ".nia-command-{}-{sequence}.committed",
            std::process::id()
        ));
        if committed_directory.exists() {
            continue;
        }
        match fs::create_dir(&directory) {
            Ok(()) => {
                let outputs = resolved_outputs
                    .iter()
                    .enumerate()
                    .map(|(index, (_, destination))| StagedOutputEntry {
                        destination: destination.clone(),
                        temporary: directory.join(format!("output-{index}")),
                        backup: directory.join(format!("backup-{index}")),
                    })
                    .collect::<Vec<_>>();
                let logical_outputs = resolved_outputs
                    .iter()
                    .map(|(output, _)| (*output).clone())
                    .collect::<Vec<_>>();
                let journal = match OutputTransactionJournal::create(
                    build_dir,
                    &action.key,
                    &logical_outputs,
                    &directory,
                    &committed_directory,
                ) {
                    Ok(journal) => journal,
                    Err(error) => {
                        let _ = fs::remove_dir_all(&directory);
                        return Err(staged_output_io(
                            action,
                            &directory,
                            "create recovery journal for",
                            error,
                            None,
                        ));
                    }
                };
                return Ok(StagedOutputTransaction {
                    directory,
                    committed_directory,
                    outputs,
                    journal,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(staged_output_io(
                    action,
                    &directory,
                    "create staging directory for",
                    error,
                    None,
                ));
            }
        }
    }
    Err(staged_output_io(
        action,
        parent,
        "create unique staging directory in",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted staged-output directory names",
        ),
        None,
    ))
}

fn publish_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
) -> Result<(), CoordinatorError> {
    publish_staged_outputs_with(action, staged, |_| Ok(()))
}

fn publish_staged_outputs_with(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    mut before_install: impl FnMut(usize) -> io::Result<()>,
) -> Result<(), CoordinatorError> {
    let prepared = (|| {
        let mut publications = Vec::with_capacity(staged.outputs.len());
        for output in &staged.outputs {
            let metadata = fs::symlink_metadata(&output.temporary).map_err(|error| {
                staged_output_io(
                    action,
                    &output.temporary,
                    "inspect command-produced",
                    error,
                    None,
                )
            })?;
            if !metadata.file_type().is_file() {
                return Err(staged_output_io(
                    action,
                    &output.temporary,
                    "publish non-file",
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "external command output must be a regular file",
                    ),
                    None,
                ));
            }
            fs::File::open(&output.temporary)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    staged_output_io(
                        action,
                        &output.temporary,
                        "sync command-produced",
                        error,
                        None,
                    )
                })?;
            let parent = output
                .destination
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .ok_or_else(|| {
                    staged_output_io(
                        action,
                        &output.destination,
                        "resolve parent for",
                        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"),
                        None,
                    )
                })?;
            fs::create_dir_all(parent).map_err(|error| {
                staged_output_io(action, parent, "create parent directory for", error, None)
            })?;
            let had_previous = match fs::symlink_metadata(&output.destination) {
                Ok(metadata) if metadata.file_type().is_file() => true,
                Ok(_) => {
                    return Err(staged_output_io(
                        action,
                        &output.destination,
                        "replace non-file",
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "accepted external command output must be a regular file",
                        ),
                        None,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(staged_output_io(
                        action,
                        &output.destination,
                        "inspect previous",
                        error,
                        None,
                    ));
                }
            };
            publications.push(StagedOutputPublication {
                had_previous,
                backed_up: false,
                installed: false,
            });
        }
        fs::File::open(&staged.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                staged_output_io(
                    action,
                    &staged.directory,
                    "sync prepared transaction",
                    error,
                    None,
                )
            })?;
        staged
            .journal
            .mark_prepared(
                &publications
                    .iter()
                    .map(|publication| publication.had_previous)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| {
                staged_output_io(
                    action,
                    staged.journal.path(),
                    "persist prepared recovery state for",
                    error,
                    None,
                )
            })?;
        Ok(publications)
    })();
    let mut publications = match prepared {
        Ok(publications) => publications,
        Err(cause) => {
            return cleanup_staged_outputs(action, staged, Some(Box::new(cause)));
        }
    };
    let committed = (|| {
        for (index, (output, publication)) in staged
            .outputs
            .iter()
            .zip(publications.iter_mut())
            .enumerate()
        {
            before_install(index).map_err(|error| {
                staged_output_io(
                    action,
                    &output.destination,
                    "commit transaction entry for",
                    error,
                    None,
                )
            })?;
            if publication.had_previous {
                fs::rename(&output.destination, &output.backup).map_err(|error| {
                    staged_output_io(action, &output.destination, "back up previous", error, None)
                })?;
                publication.backed_up = true;
            }
            fs::rename(&output.temporary, &output.destination).map_err(|error| {
                staged_output_io(action, &output.destination, "install", error, None)
            })?;
            publication.installed = true;
        }
        let parents: BTreeSet<_> = staged
            .outputs
            .iter()
            .filter_map(|output| output.destination.parent())
            .collect();
        for parent in parents {
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    staged_output_io(
                        action,
                        parent,
                        "sync committed output directory for",
                        error,
                        None,
                    )
                })?;
        }
        fs::rename(&staged.directory, &staged.committed_directory).map_err(|error| {
            staged_output_io(
                action,
                &staged.directory,
                "mark transaction committed at",
                error,
                None,
            )
        })
    })();
    match committed {
        Ok(()) => {
            if let Some(parent) = staged.committed_directory.parent() {
                let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
            }
            let committed_cleaned = match fs::remove_dir_all(&staged.committed_directory) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                Err(_) => false,
            };
            if let Some(parent) = staged.committed_directory.parent() {
                let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
            }
            if committed_cleaned {
                let _ = staged.journal.cleanup();
            }
            Ok(())
        }
        Err(cause) => rollback_staged_outputs(action, staged, publications, cause),
    }
}

fn rollback_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    publications: Vec<StagedOutputPublication>,
    cause: CoordinatorError,
) -> Result<(), CoordinatorError> {
    let mut cause = Some(Box::new(cause));
    for (output, publication) in staged.outputs.iter().zip(&publications).rev() {
        if publication.installed
            && let Err(error) = fs::rename(&output.destination, &output.temporary)
        {
            return Err(staged_output_io(
                action,
                &output.destination,
                "roll back installed",
                error,
                cause.take(),
            ));
        }
        if publication.backed_up
            && let Err(error) = fs::rename(&output.backup, &output.destination)
        {
            return Err(staged_output_io(
                action,
                &output.destination,
                "restore previous",
                error,
                cause.take(),
            ));
        }
    }
    let parents: BTreeSet<_> = staged
        .outputs
        .iter()
        .filter_map(|output| output.destination.parent())
        .collect();
    for parent in parents {
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            return Err(staged_output_io(
                action,
                parent,
                "sync rolled-back output directory for",
                error,
                cause.take(),
            ));
        }
    }
    cleanup_staged_outputs(action, staged, cause)
}

fn cleanup_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    cause: Option<Box<CoordinatorError>>,
) -> Result<(), CoordinatorError> {
    match fs::remove_dir_all(&staged.directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(staged_output_io(
                action,
                &staged.directory,
                "clean up",
                error,
                cause,
            ));
        }
    }
    if let Some(parent) = staged.directory.parent()
        && let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all())
    {
        return Err(staged_output_io(
            action,
            parent,
            "sync cleanup directory for",
            error,
            cause,
        ));
    }
    if let Err(error) = staged.journal.cleanup() {
        return Err(staged_output_io(
            action,
            staged.journal.path(),
            "clean up recovery journal for",
            error,
            cause,
        ));
    }
    match cause {
        Some(cause) => Err(*cause),
        None => Ok(()),
    }
}

fn staged_output_io(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
    error: io::Error,
    cause: Option<Box<CoordinatorError>>,
) -> CoordinatorError {
    CoordinatorError::StagedOutput {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
        cause,
    }
}

static GENERATED_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn write_generated_file(
    action: &PlanAction,
    output: &std::path::Path,
    contents: &[u8],
) -> Result<(), CoordinatorError> {
    if fs::symlink_metadata(output).is_ok_and(|metadata| metadata.file_type().is_file())
        && fs::read(output).is_ok_and(|current| current == contents)
    {
        return Ok(());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            generated_io(
                action,
                output,
                "resolve parent for",
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated output has no parent",
                ),
            )
        })?;
    fs::create_dir_all(parent)
        .map_err(|error| generated_io(action, parent, "create parent directory for", error))?;
    let (temporary_path, mut temporary) = create_generated_temporary(action, parent)?;
    let result = (|| {
        temporary
            .write_all(contents)
            .map_err(|error| generated_io(action, &temporary_path, "write", error))?;
        temporary
            .sync_all()
            .map_err(|error| generated_io(action, &temporary_path, "sync", error))?;
        drop(temporary);
        fs::rename(&temporary_path, output)
            .map_err(|error| generated_io(action, output, "publish", error))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| generated_io(action, parent, "sync parent directory for", error))
    })();
    if result.is_err() {
        match fs::remove_file(&temporary_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(generated_io(
                    action,
                    &temporary_path,
                    "clean up temporary",
                    error,
                ));
            }
        }
    }
    result
}

fn create_generated_temporary(
    action: &PlanAction,
    parent: &std::path::Path,
) -> Result<(PathBuf, fs::File), CoordinatorError> {
    for _ in 0..128 {
        let sequence = GENERATED_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".nia-generated-{}-{sequence}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(generated_io(action, &path, "create temporary", error)),
        }
    }
    Err(generated_io(
        action,
        parent,
        "create unique temporary in",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted generated-file temporary names",
        ),
    ))
}

fn generated_io(
    action: &PlanAction,
    path: &std::path::Path,
    operation: &'static str,
    error: io::Error,
) -> CoordinatorError {
    CoordinatorError::GeneratedFileIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    }
}

fn validate_invocation_targets(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<(), CoordinatorError> {
    for (role, found, expected) in [
        (
            "host",
            plan.host_target(),
            target_spec(invocation.toolchain.host_target()),
        ),
        (
            "artifact",
            plan.artifact_target(),
            target_spec(invocation.toolchain.artifact_target()),
        ),
    ] {
        if found != &expected {
            return Err(CoordinatorError::TargetMismatch(Box::new(TargetMismatch {
                role,
                expected,
                found: found.clone(),
            })));
        }
    }
    Ok(())
}

fn find_step(steps: &[crate::PlanStep], key: &StepKey) -> Option<usize> {
    steps.binary_search_by(|step| step.key.cmp(key)).ok()
}

fn find_action<'a>(actions: &'a [PlanAction], key: &ActionKey) -> Option<&'a PlanAction> {
    actions
        .binary_search_by(|action| action.key.cmp(key))
        .ok()
        .map(|index| &actions[index])
}

fn find_module<'a>(modules: &'a [PlanModule], key: &ModuleKey) -> Option<&'a PlanModule> {
    modules
        .binary_search_by(|module| module.key.cmp(key))
        .ok()
        .map(|index| &modules[index])
}

fn find_artifact<'a>(artifacts: &'a [PlanArtifact], key: &ArtifactKey) -> Option<&'a PlanArtifact> {
    artifacts
        .binary_search_by(|artifact| artifact.key.cmp(key))
        .ok()
        .map(|index| &artifacts[index])
}

fn inconsistent(owner: impl Into<String>, missing: String) -> CoordinatorError {
    CoordinatorError::InconsistentPlan {
        owner: owner.into(),
        missing,
    }
}

fn unsupported(action: &PlanAction, kind: &'static str) -> CoordinatorError {
    CoordinatorError::UnsupportedAction {
        action: action.key.clone(),
        kind,
    }
}

fn optimization(mode: OptimizationMode) -> NiaOptimizationLevel {
    match mode {
        OptimizationMode::O0 => NiaOptimizationLevel::O0,
        OptimizationMode::O1 => NiaOptimizationLevel::O1,
        OptimizationMode::O2 => NiaOptimizationLevel::O2,
        OptimizationMode::O3 => NiaOptimizationLevel::O3,
        OptimizationMode::Os => NiaOptimizationLevel::Os,
        OptimizationMode::Oz => NiaOptimizationLevel::Oz,
    }
}

fn runtime_mode(runtime: Runtime) -> DriverRuntime {
    match runtime {
        Runtime::Bare => DriverRuntime::Bare,
        Runtime::Freestanding => DriverRuntime::Freestanding,
    }
}

fn target_spec(target: &TargetConfig) -> TargetSpec {
    TargetSpec {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

fn target_config(target: &TargetSpec) -> TargetConfig {
    TargetConfig {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

fn display_target(target: &TargetSpec) -> String {
    format!(
        "{}-{}-{}-{}-{} ({}-bit {})",
        target.arch,
        target.vendor,
        target.os,
        target.env,
        target.abi,
        target.pointer_width,
        target.endian
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildPlanDraft, ModuleImport, PlanAction, PlanPackage, PlanStep};
    use std::sync::{Arc, Barrier, Mutex, OnceLock, atomic::AtomicBool};

    fn action(name: &str) -> ActionKey {
        ActionKey::new(PackageKey::root(), name).unwrap()
    }

    fn step(name: &str) -> StepKey {
        StepKey::new(PackageKey::root(), name).unwrap()
    }

    fn target() -> TargetSpec {
        TargetSpec {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: String::new(),
            abi: String::new(),
            endian: "little".to_string(),
            pointer_width: 64,
        }
    }

    fn test_invocation() -> BuildInvocation {
        static TOOLCHAIN: OnceLock<Arc<nia_toolchain::ToolchainLayout>> = OnceLock::new();
        static TEST_RUN_ID: OnceLock<u128> = OnceLock::new();
        let toolchain = Arc::clone(TOOLCHAIN.get_or_init(|| {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace = manifest_dir
                .parent()
                .and_then(std::path::Path::parent)
                .expect("nia-build lives under crates");
            Arc::new(
                nia_toolchain::ToolchainLayout::resolve(
                    nia_toolchain::ToolchainLayoutRequest::explicit(
                        std::env::current_exe().expect("test executable"),
                        workspace.join("lib"),
                    ),
                )
                .expect("test toolchain"),
            )
        }));
        let test_run_id = TEST_RUN_ID.get_or_init(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock must follow the Unix epoch")
                .as_nanos()
        });
        let sequence = GENERATED_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let package_root = std::env::temp_dir().join(format!(
            "nia-build-coordinator-test-{}-{test_run_id}-{sequence}",
            std::process::id(),
        ));
        let build_dir = package_root.join(".nia-build");
        BuildInvocation {
            toolchain,
            build_script: package_root.join("build.nia"),
            cache_dir: package_root.join(".nia-cache"),
            runner_dir: build_dir.join("runner"),
            runner_executable: build_dir.join("runner/nia-build-runner"),
            plan_draft: build_dir.join("build-plan.draft"),
            plan_path: build_dir.join("build-plan.bin"),
            build_dir,
            package_root,
            step: crate::BuildStepSelection::Default,
            timings: nia_driver::TimingMode::Off,
            timing_format: nia_timing::TimingFormat::Text,
            max_parallel_actions: None,
        }
    }

    fn aggregate_plan(
        actions: &[&str],
        steps: Vec<(&str, &str, Vec<&str>)>,
        selected: &str,
    ) -> BuildPlan {
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: actions
                .iter()
                .map(|name| PlanAction {
                    key: action(name),
                    kind: ActionKind::Aggregate,
                })
                .collect(),
            steps: steps
                .into_iter()
                .map(|(name, action_name, dependencies)| PlanStep {
                    key: step(name),
                    action: action(action_name),
                    dependencies: dependencies.into_iter().map(step).collect(),
                })
                .collect(),
            default_step: None,
            selected_step: Some(step(selected)),
        })
        .unwrap()
    }

    #[test]
    fn compiler_requests_preserve_physical_paths_and_stable_build_identities() {
        let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
        let check = action("check");
        let check_step = step("check");
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: vec![PlanModule {
                key: module.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/main.nia",
                )
                .unwrap(),
                optimization: OptimizationMode::O2,
                imports: vec![ModuleImport {
                    name: "generated".to_string(),
                    path: LogicalPath::new(LogicalPathRoot::Build, "generated/root.nia").unwrap(),
                }],
            }],
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: check.clone(),
                kind: ActionKind::CompilerCheck {
                    module: module.clone(),
                    target: target(),
                    runtime: Runtime::Freestanding,
                },
            }],
            steps: vec![PlanStep {
                key: check_step.clone(),
                action: check,
                dependencies: Vec::new(),
            }],
            default_step: Some(check_step.clone()),
            selected_step: Some(check_step),
        })
        .unwrap();
        let invocation = test_invocation();
        let executor = DriverActionExecutor::new(plan.clone(), invocation.clone());
        let request = executor
            .check_request(&plan.actions()[0], &module, Runtime::Freestanding)
            .expect("construct compiler check request");
        let generated = request
            .module_map
            .get("generated")
            .expect("generated module mapping");

        assert_eq!(
            request.entry_path.as_str(),
            invocation
                .package_root
                .join("src/main.nia")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            request.entry_path.identity().normalized_path(),
            "build-package:root:/src/main.nia"
        );
        assert_eq!(
            generated.as_str(),
            invocation
                .build_dir
                .join("generated/root.nia")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            generated.identity().normalized_path(),
            "build-output:root:/generated/root.nia"
        );
    }

    #[test]
    fn selected_closure_is_iterative_deterministic_and_excludes_unselected_steps() {
        let plan = aggregate_plan(
            &["shared", "left", "right", "final", "unused"],
            vec![
                ("unused", "unused", vec![]),
                ("final", "final", vec!["right", "left"]),
                ("right", "right", vec!["shared"]),
                ("left", "left", vec!["shared"]),
                ("shared", "shared", vec![]),
            ],
            "final",
        );
        let mut observed = Vec::new();
        let mut waves = Vec::new();

        let report = execute_selected_closure(&plan, |items| {
            observed.extend(items.iter().map(|item| item.key.name().to_string()));
            waves.push(
                items
                    .iter()
                    .map(|item| item.key.name().to_string())
                    .collect::<Vec<_>>(),
            );
            items
                .iter()
                .map(|_| ActionOutcome::Succeeded(None))
                .collect()
        })
        .unwrap();

        assert_eq!(observed, ["shared", "left", "right", "final"]);
        assert_eq!(
            waves,
            [
                vec!["shared".to_string()],
                vec!["left".to_string(), "right".to_string()],
                vec!["final".to_string()],
            ]
        );
        assert_eq!(
            report.steps.iter().map(StepKey::name).collect::<Vec<_>>(),
            ["shared", "left", "right", "final"]
        );
    }

    #[test]
    fn shared_action_executes_once_across_multiple_steps() {
        let plan = aggregate_plan(
            &["shared", "final"],
            vec![
                ("a", "shared", vec![]),
                ("b", "shared", vec![]),
                ("final", "final", vec!["a", "b"]),
            ],
            "final",
        );
        let mut observed = Vec::new();

        let report = execute_selected_closure(&plan, |items| {
            observed.extend(items.iter().map(|item| item.key.name().to_string()));
            items
                .iter()
                .map(|_| ActionOutcome::Succeeded(None))
                .collect()
        })
        .unwrap();

        assert_eq!(observed, ["shared", "final"]);
        assert_eq!(report.steps.len(), 3);
        assert_eq!(report.actions.len(), 2);
    }

    #[test]
    fn action_failure_stops_dependents_and_retains_action_context() {
        let plan = aggregate_plan(
            &["first", "second"],
            vec![
                ("first", "first", vec![]),
                ("second", "second", vec!["first"]),
            ],
            "second",
        );

        let error = execute_selected_closure(&plan, |items| {
            items
                .iter()
                .map(|item| ActionOutcome::Failed(unsupported(item, "test-failure")))
                .collect()
        })
        .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "test-failure" }
                if action.name() == "first"
        ));
    }

    #[test]
    fn cancellation_preserves_earlier_canonical_failure_candidates() {
        let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
        let earlier = ActionCancellation {
            earliest_failure: Arc::clone(&earliest_failure),
            position: 0,
        };
        let first_failure = ActionCancellation {
            earliest_failure: Arc::clone(&earliest_failure),
            position: 1,
        };
        let later = ActionCancellation {
            earliest_failure,
            position: 2,
        };

        first_failure.cancel_later_actions();

        assert!(!earlier.is_cancelled());
        assert!(!first_failure.is_cancelled());
        assert!(later.is_cancelled());
        earlier.cancel_later_actions();
        assert!(first_failure.is_cancelled());
    }

    #[test]
    fn concurrent_completion_order_does_not_change_visible_order() {
        let plan = aggregate_plan(
            &["shared", "left", "right", "final"],
            vec![
                ("final", "final", vec!["left", "right"]),
                ("right", "right", vec!["shared"]),
                ("left", "left", vec!["shared"]),
                ("shared", "shared", vec![]),
            ],
            "final",
        );
        let sequential = execute_selected_closure(&plan, |items| {
            items
                .iter()
                .map(|_| ActionOutcome::Succeeded(None))
                .collect()
        })
        .unwrap();
        let completion_order = Arc::new(Mutex::new(Vec::new()));

        let concurrent = execute_selected_closure(&plan, |items| {
            let barrier = Arc::new(Barrier::new(items.len()));
            std::thread::scope(|scope| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let barrier = Arc::clone(&barrier);
                        let completion_order = Arc::clone(&completion_order);
                        scope.spawn(move || {
                            barrier.wait();
                            if items.len() > 1 && index == 0 {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            completion_order
                                .lock()
                                .expect("completion order lock poisoned")
                                .push(item.key.name().to_string());
                            ActionOutcome::Succeeded(None)
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().expect("synthetic action worker"))
                    .collect()
            })
        })
        .unwrap();

        let completion_order = completion_order
            .lock()
            .expect("completion order lock poisoned");
        let left = completion_order
            .iter()
            .position(|name| name == "left")
            .unwrap();
        let right = completion_order
            .iter()
            .position(|name| name == "right")
            .unwrap();
        assert!(right < left);
        assert_eq!(concurrent, sequential);
        assert_eq!(
            concurrent
                .actions
                .iter()
                .map(ActionKey::name)
                .collect::<Vec<_>>(),
            ["shared", "left", "right", "final"]
        );
    }

    #[test]
    fn single_worker_action_limit_serializes_ready_tasks() {
        let session = QuerySession::new();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks = (0..64).map(|value| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            (ActionResourceClass::Cpu, move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                std::thread::yield_now();
                active.fetch_sub(1, Ordering::SeqCst);
                value
            })
        });

        let values = run_action_tasks(
            &session,
            Some(std::num::NonZeroUsize::new(1).unwrap()),
            tasks,
        );

        assert_eq!(values, (0..64).collect::<Vec<_>>());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn action_resource_capacity_only_reduces_inherited_capacity() {
        let session = QuerySession::new();
        let inherited = session.executor_parallelism();
        assert_eq!(action_resource_capacity(&session, None), inherited);
        assert_eq!(
            action_resource_capacity(
                &session,
                std::num::NonZeroUsize::new(inherited.saturating_add(8))
            ),
            inherited
        );
        assert_eq!(
            action_resource_capacity(&session, std::num::NonZeroUsize::new(1)),
            1
        );
    }

    #[test]
    fn bounded_wide_graph_stress_preserves_deterministic_report() {
        let width = 48usize;
        let leaves = (0..width)
            .map(|index| format!("leaf-{index:02}"))
            .collect::<Vec<_>>();
        let mut actions = leaves
            .iter()
            .map(|name| PlanAction {
                key: action(name),
                kind: ActionKind::Aggregate,
            })
            .collect::<Vec<_>>();
        actions.push(PlanAction {
            key: action("final"),
            kind: ActionKind::Aggregate,
        });
        let mut steps = leaves
            .iter()
            .map(|name| PlanStep {
                key: step(name),
                action: action(name),
                dependencies: Vec::new(),
            })
            .collect::<Vec<_>>();
        steps.push(PlanStep {
            key: step("final"),
            action: action("final"),
            dependencies: leaves.iter().map(|name| step(name)).collect(),
        });
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions,
            steps,
            default_step: None,
            selected_step: Some(step("final")),
        })
        .unwrap();
        let expected = execute_selected_closure(&plan, |items| {
            items
                .iter()
                .map(|_| ActionOutcome::Succeeded(None))
                .collect()
        })
        .unwrap();

        for iteration in 0..32usize {
            let session = QuerySession::new();
            let limit = std::num::NonZeroUsize::new(if iteration % 4 == 0 { 1 } else { 4 });
            let report = execute_selected_closure(&plan, |items| {
                let tasks = items
                    .iter()
                    .enumerate()
                    .map(|(position, _)| {
                        (
                            if position % 2 == 0 {
                                ActionResourceClass::Cpu
                            } else {
                                ActionResourceClass::Io
                            },
                            move || {
                                for _ in 0..(position + iteration) % 7 {
                                    std::thread::yield_now();
                                }
                                ActionOutcome::Succeeded(None)
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                run_action_tasks(&session, limit, tasks)
            })
            .unwrap();

            assert_eq!(report, expected, "iteration {iteration}");
        }
    }

    #[test]
    fn failure_waits_for_active_wave_and_never_dispatches_dependents() {
        let plan = aggregate_plan(
            &["active", "active-next", "fail", "blocked", "final"],
            vec![
                ("active", "active", vec![]),
                ("fail", "fail", vec![]),
                ("active-next", "active-next", vec!["active"]),
                ("blocked", "blocked", vec!["fail"]),
                ("final", "final", vec!["active-next", "blocked"]),
            ],
            "final",
        );
        let active_settled = Arc::new(AtomicBool::new(false));
        let mut dispatched = Vec::new();

        let error = execute_selected_closure(&plan, |items| {
            dispatched.push(
                items
                    .iter()
                    .map(|item| item.key.name().to_string())
                    .collect::<Vec<_>>(),
            );
            let barrier = Arc::new(Barrier::new(items.len()));
            std::thread::scope(|scope| {
                items
                    .iter()
                    .map(|item| {
                        let barrier = Arc::clone(&barrier);
                        let active_settled = Arc::clone(&active_settled);
                        scope.spawn(move || {
                            barrier.wait();
                            if item.key.name() == "fail" {
                                ActionOutcome::Failed(unsupported(item, "wave-failure"))
                            } else {
                                std::thread::sleep(Duration::from_millis(25));
                                active_settled.store(true, Ordering::Release);
                                ActionOutcome::Succeeded(None)
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().expect("synthetic action worker"))
                    .collect()
            })
        })
        .unwrap_err();

        assert!(active_settled.load(Ordering::Acquire));
        assert_eq!(dispatched, [vec!["active".to_string(), "fail".to_string()]]);
        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "wave-failure" }
                if action.name() == "fail"
        ));
    }

    #[test]
    fn empty_plan_executes_nothing() {
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            default_step: None,
            selected_step: None,
        })
        .unwrap();

        let report = execute_selected_closure(&plan, |_| unreachable!()).unwrap();
        assert!(report.steps.is_empty());
        assert!(report.actions.is_empty());
    }

    #[test]
    fn all_optimization_and_runtime_modes_map_exactly() {
        assert_eq!(optimization(OptimizationMode::O0), NiaOptimizationLevel::O0);
        assert_eq!(optimization(OptimizationMode::O1), NiaOptimizationLevel::O1);
        assert_eq!(optimization(OptimizationMode::O2), NiaOptimizationLevel::O2);
        assert_eq!(optimization(OptimizationMode::O3), NiaOptimizationLevel::O3);
        assert_eq!(optimization(OptimizationMode::Os), NiaOptimizationLevel::Os);
        assert_eq!(optimization(OptimizationMode::Oz), NiaOptimizationLevel::Oz);
        assert_eq!(runtime_mode(Runtime::Bare), DriverRuntime::Bare);
        assert_eq!(
            runtime_mode(Runtime::Freestanding),
            DriverRuntime::Freestanding
        );
    }

    #[test]
    fn explicit_uncacheable_action_has_no_legacy_execution_fallback() {
        let invocation = test_invocation();
        let target = target_spec(invocation.toolchain.host_target());
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target.clone(),
            artifact_target: target,
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("opaque"),
                kind: ActionKind::Uncacheable {
                    description: "legacy callback".to_string(),
                },
            }],
            steps: vec![PlanStep {
                key: step("opaque"),
                action: action("opaque"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("opaque")),
            selected_step: None,
        })
        .unwrap();

        let error = execute_build_plan(&plan, &invocation).unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "uncacheable" }
                if action.name() == "opaque" && action.package().as_str() == "root"
        ));
    }

    #[test]
    fn invocation_target_mismatch_is_rejected_before_actions() {
        let invocation = test_invocation();
        let mut mismatched = target_spec(invocation.toolchain.host_target());
        mismatched.arch = "mismatched".to_string();
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: mismatched,
            artifact_target: target_spec(invocation.toolchain.artifact_target()),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            default_step: None,
            selected_step: None,
        })
        .unwrap();

        let error = execute_build_plan(&plan, &invocation).unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::TargetMismatch(details)
                if details.role == "host" && details.found.arch == "mismatched"
        ));
    }

    fn generated_plan(invocation: &BuildInvocation, output: &str, contents: &[u8]) -> BuildPlan {
        let host = target_spec(invocation.toolchain.host_target());
        let artifact = target_spec(invocation.toolchain.artifact_target());
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: host,
            artifact_target: artifact,
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("generate"),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, output).unwrap(),
                    contents: contents.to_vec(),
                },
            }],
            steps: vec![PlanStep {
                key: step("generate"),
                action: action("generate"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("generate")),
            selected_step: None,
        })
        .unwrap()
    }

    fn compiler_check_plan(
        invocation: &BuildInvocation,
        optimization: OptimizationMode,
    ) -> BuildPlan {
        compiler_check_plan_with_runtime(invocation, optimization, Runtime::Bare)
    }

    fn compiler_check_plan_with_runtime(
        invocation: &BuildInvocation,
        optimization: OptimizationMode,
        runtime: Runtime,
    ) -> BuildPlan {
        let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
        let host = target_spec(invocation.toolchain.host_target());
        let artifact = target_spec(invocation.toolchain.artifact_target());
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: host,
            artifact_target: artifact.clone(),
            modules: vec![PlanModule {
                key: module.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/main.nia",
                )
                .unwrap(),
                optimization,
                imports: Vec::new(),
            }],
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("check"),
                kind: ActionKind::CompilerCheck {
                    module,
                    target: artifact,
                    runtime,
                },
            }],
            steps: vec![PlanStep {
                key: step("check"),
                action: action("check"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("check")),
            selected_step: None,
        })
        .unwrap()
    }

    fn compiler_emit_plan(
        invocation: &BuildInvocation,
        optimization: OptimizationMode,
        runtime: Runtime,
        output: &str,
    ) -> BuildPlan {
        let module = ModuleKey::new(PackageKey::root(), "app").unwrap();
        let artifact_key = ArtifactKey::new(PackageKey::root(), "app").unwrap();
        let host = target_spec(invocation.toolchain.host_target());
        let artifact_target = target_spec(invocation.toolchain.artifact_target());
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: host,
            artifact_target: artifact_target.clone(),
            modules: vec![PlanModule {
                key: module.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/main.nia",
                )
                .unwrap(),
                optimization,
                imports: Vec::new(),
            }],
            artifacts: vec![PlanArtifact {
                key: artifact_key.clone(),
                root_module: module,
                output: LogicalPath::new(LogicalPathRoot::Build, output).unwrap(),
                runtime,
            }],
            actions: vec![PlanAction {
                key: action("emit"),
                kind: ActionKind::CompilerEmit {
                    artifact: artifact_key,
                    target: artifact_target,
                },
            }],
            steps: vec![PlanStep {
                key: step("emit"),
                action: action("emit"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("emit")),
            selected_step: None,
        })
        .unwrap()
    }

    fn write_compiler_check_source(invocation: &BuildInvocation, source: &str) {
        let path = invocation.package_root.join("src/main.nia");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn only_compiler_check_outcome(report: &ExecutionReport) -> &ActionCacheOutcome {
        assert_eq!(report.action_cache.len(), 1);
        assert_eq!(report.action_cache[0].action, action("check"));
        &report.action_cache[0].outcome
    }

    fn only_compiler_emit_outcome(report: &ExecutionReport) -> &ActionCacheOutcome {
        assert_eq!(report.action_cache.len(), 1);
        assert_eq!(report.action_cache[0].action, action("emit"));
        &report.action_cache[0].outcome
    }

    fn freestanding_source(body: &str) -> String {
        [
            "using std::process;\n",
            "pub fn main(init: process::Init) process::ExitCode!void {\n",
            "    _ = init;\n",
            "    ",
            body,
            "\n}\n",
        ]
        .concat()
    }

    fn only_nested_cache_entry(namespace: &Path, extension: &str) -> PathBuf {
        let key_dir = fs::read_dir(namespace)
            .expect("read cache namespace")
            .next()
            .expect("cache key directory")
            .expect("read cache key directory")
            .path();
        fs::read_dir(key_dir)
            .expect("read cache key entries")
            .find_map(|entry| {
                let path = entry.expect("read cache entry").path();
                (path.extension().and_then(|value| value.to_str()) == Some(extension))
                    .then_some(path)
            })
            .expect("cache entry")
    }

    fn assert_no_output_locks(invocation: &BuildInvocation) {
        let root = invocation.cache_dir.join("coordination/output-locks");
        if root.is_dir() {
            assert!(fs::read_dir(root).unwrap().next().is_none());
        }
    }

    #[test]
    fn output_lock_keys_are_path_stable_and_domain_local() {
        let invocation = test_invocation();
        let first = LogicalPath::new(LogicalPathRoot::Build, "first/output").unwrap();
        let same = LogicalPath::new(LogicalPathRoot::Build, "first/output").unwrap();
        let second = LogicalPath::new(LogicalPathRoot::Build, "second/output").unwrap();

        assert_eq!(
            output_lock_path(&invocation.cache_dir, &first),
            output_lock_path(&invocation.cache_dir, &same)
        );
        assert_ne!(
            output_lock_path(&invocation.cache_dir, &first),
            output_lock_path(&invocation.cache_dir, &second)
        );
        assert!(
            output_lock_path(&invocation.cache_dir, &first)
                .starts_with(invocation.cache_dir.join("coordination/output-locks"))
        );
    }

    #[test]
    fn output_coordination_serializes_conflicts_only() {
        let invocation = test_invocation();
        let blocked_output =
            LogicalPath::new(LogicalPathRoot::Build, "blocked/output.txt").unwrap();
        let held =
            ScopedFileLock::acquire(output_lock_path(&invocation.cache_dir, &blocked_output))
                .unwrap();

        let independent = generated_plan(&invocation, "independent/output.txt", b"independent");
        execute_build_plan(&independent, &invocation).unwrap();

        let conflicting = generated_plan(&invocation, "blocked/output.txt", b"conflicting");
        let concurrent_invocation = invocation.clone();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = execute_build_plan(&conflicting, &concurrent_invocation);
            finished_tx.send(result).unwrap();
        });

        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(held);
        finished_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        handle.join().unwrap();
        assert_eq!(
            fs::read(invocation.build_dir.join("blocked/output.txt")).unwrap(),
            b"conflicting"
        );
        assert_no_output_locks(&invocation);
    }

    #[test]
    fn output_coordination_failure_retains_action_and_paths() {
        let invocation = test_invocation();
        fs::create_dir_all(invocation.cache_dir.join("coordination")).unwrap();
        fs::write(
            invocation.cache_dir.join("coordination/output-locks"),
            b"occupied",
        )
        .unwrap();
        let plan = generated_plan(&invocation, "generated/output.txt", b"contents");

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::AcquireOutputLock {
                action,
                output,
                lock,
                ..
            } if action.name() == "generate"
                && output == invocation.build_dir.join("generated/output.txt")
                && lock.starts_with(invocation.cache_dir.join("coordination/output-locks"))
        ));
    }

    #[test]
    fn generated_file_replaces_previous_content_atomically() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("generated/source.nia");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"old").unwrap();
        let plan = generated_plan(&invocation, "generated/source.nia", b"new contents");

        let report = execute_build_plan(&plan, &invocation).unwrap();

        assert_eq!(report.actions, [action("generate")]);
        assert_eq!(fs::read(&output).unwrap(), b"new contents");
        assert_no_output_locks(&invocation);
        assert!(
            fs::read_dir(output.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".nia-generated-"))
        );
    }

    #[test]
    fn generated_file_cache_reports_miss_hit_and_restores_output() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("generated/source.nia");
        let plan = generated_plan(&invocation, "generated/source.nia", b"source");

        let cold = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            cold.action_cache,
            [ActionCacheReport {
                action: action("generate"),
                outcome: ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound),
            }]
        );
        fs::remove_file(&output).unwrap();

        let warm = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            warm.action_cache,
            [ActionCacheReport {
                action: action("generate"),
                outcome: ActionCacheOutcome::Hit,
            }]
        );
        assert_eq!(fs::read(output).unwrap(), b"source");
        assert_no_output_locks(&invocation);
    }

    #[test]
    fn generated_file_cache_reports_content_and_output_invalidation() {
        let invocation = test_invocation();
        let baseline = generated_plan(&invocation, "generated/source.nia", b"source");
        execute_build_plan(&baseline, &invocation).unwrap();

        let changed_contents =
            generated_plan(&invocation, "generated/source.nia", b"changed source");
        let content_report = execute_build_plan(&changed_contents, &invocation).unwrap();
        assert_eq!(
            content_report.action_cache[0].outcome,
            ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Contents,
            ]))
        );

        let other_invocation = test_invocation();
        let first_output = generated_plan(&other_invocation, "generated/first.nia", b"source");
        execute_build_plan(&first_output, &other_invocation).unwrap();
        let changed_output = generated_plan(&other_invocation, "generated/second.nia", b"source");
        let output_report = execute_build_plan(&changed_output, &other_invocation).unwrap();
        assert_eq!(
            output_report.action_cache[0].outcome,
            ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Output,
            ]))
        );
    }

    #[test]
    fn compiler_check_cache_reports_cold_and_warm_execution() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
        let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

        let cold = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_check_outcome(&cold),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
        );

        let warm = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
    }

    #[test]
    fn compiler_check_cache_reports_source_and_optimization_invalidation() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
        execute_build_plan(
            &compiler_check_plan(&invocation, OptimizationMode::O2),
            &invocation,
        )
        .unwrap();

        write_compiler_check_source(&invocation, "fn main() i32 { 1 }");
        let changed_source = execute_build_plan(
            &compiler_check_plan(&invocation, OptimizationMode::O2),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_check_outcome(&changed_source),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Sources,
            ]))
        );

        let changed_optimization = execute_build_plan(
            &compiler_check_plan(&invocation, OptimizationMode::O0),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_check_outcome(&changed_optimization),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Optimization,
            ]))
        );
    }

    #[test]
    fn compiler_check_cache_reuses_relocated_sources() {
        let first = test_invocation();
        write_compiler_check_source(&first, "fn main() i32 { 0 }");
        execute_build_plan(&compiler_check_plan(&first, OptimizationMode::O2), &first).unwrap();

        let mut relocated = test_invocation();
        relocated.cache_dir = first.cache_dir.clone();
        write_compiler_check_source(&relocated, "fn main() i32 { 0 }");
        let report = execute_build_plan(
            &compiler_check_plan(&relocated, OptimizationMode::O2),
            &relocated,
        )
        .unwrap();

        assert_eq!(
            only_compiler_check_outcome(&report),
            &ActionCacheOutcome::Hit
        );
    }

    #[test]
    fn compiler_check_cache_restores_semantic_provider_source_closure() {
        let invocation = test_invocation();
        write_compiler_check_source(
            &invocation,
            concat!(
                "using std::process;\n",
                "pub fn main(init: process::Init) process::ExitCode!void {\n",
                "    _ = init;\n",
                "    !{}\n",
                "}\n",
            ),
        );
        let plan = compiler_check_plan_with_runtime(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
        );

        let cold = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_check_outcome(&cold),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
        );
        let warm = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
    }

    #[test]
    fn compiler_check_cache_does_not_publish_warnings() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, "using std::collections;\nfn main() void {}\n");
        let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

        for report in [
            execute_build_plan(&plan, &invocation).unwrap(),
            execute_build_plan(&plan, &invocation).unwrap(),
        ] {
            assert_eq!(
                only_compiler_check_outcome(&report),
                &ActionCacheOutcome::Miss(ActionCacheMissReason::Uncacheable)
            );
        }
    }

    #[test]
    fn compiler_check_cache_retires_corruption_and_recompiles() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, "fn main() i32 { 0 }");
        let plan = compiler_check_plan(&invocation, OptimizationMode::O2);
        execute_build_plan(&plan, &invocation).unwrap();
        let namespace = invocation.cache_dir.join("actions/compiler-checks/v1");
        let key_dir = fs::read_dir(namespace)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let entry = fs::read_dir(key_dir)
            .unwrap()
            .find_map(|entry| {
                let path = entry.unwrap().path();
                (path.extension().and_then(|value| value.to_str()) == Some("entry")).then_some(path)
            })
            .unwrap();
        fs::write(entry, b"corrupt").unwrap();

        let report = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_check_outcome(&report),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
        );
        let warm = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(only_compiler_check_outcome(&warm), &ActionCacheOutcome::Hit);
    }

    #[test]
    fn compiler_check_cache_never_publishes_missing_sources() {
        let invocation = test_invocation();
        let plan = compiler_check_plan(&invocation, OptimizationMode::O2);

        for _ in 0..2 {
            assert!(matches!(
                execute_build_plan(&plan, &invocation),
                Err(CoordinatorError::Driver { .. })
            ));
        }
        assert!(
            !invocation
                .cache_dir
                .join("actions/compiler-checks/v1")
                .exists()
        );
    }

    #[test]
    fn compiler_emit_cache_reports_cold_hit_and_restores_deleted_output() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, &freestanding_source("!{}"));
        let plan = compiler_emit_plan(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        );
        let output = invocation.build_dir.join("bin/app");

        let cold = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&cold),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
        );
        let expected = fs::read(&output).expect("read cold executable");
        fs::remove_file(&output).expect("remove cold executable");

        let warm = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(only_compiler_emit_outcome(&warm), &ActionCacheOutcome::Hit);
        assert_eq!(
            fs::read(output).expect("read restored executable"),
            expected
        );
    }

    #[test]
    fn compiler_emit_cache_classifies_source_option_output_and_artifact_changes() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, &freestanding_source("!{}"));
        execute_build_plan(
            &compiler_emit_plan(
                &invocation,
                OptimizationMode::O2,
                Runtime::Freestanding,
                "bin/app",
            ),
            &invocation,
        )
        .unwrap();

        write_compiler_check_source(
            &invocation,
            &freestanding_source("let value = 1; _ = value; !{}"),
        );
        let changed_source = execute_build_plan(
            &compiler_emit_plan(
                &invocation,
                OptimizationMode::O2,
                Runtime::Freestanding,
                "bin/app",
            ),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&changed_source),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Sources,
            ]))
        );

        let changed_optimization = execute_build_plan(
            &compiler_emit_plan(
                &invocation,
                OptimizationMode::O0,
                Runtime::Freestanding,
                "bin/app",
            ),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&changed_optimization),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Optimization,
            ]))
        );

        let changed_output = execute_build_plan(
            &compiler_emit_plan(
                &invocation,
                OptimizationMode::O0,
                Runtime::Freestanding,
                "bin/other",
            ),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&changed_output),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Output,
            ]))
        );

        let changed_artifact = execute_build_plan(
            &compiler_emit_plan(
                &invocation,
                OptimizationMode::O0,
                Runtime::Bare,
                "bin/other",
            ),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&changed_artifact),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Invalidated(vec![
                crate::ActionCacheInvalidation::Artifact,
            ]))
        );
    }

    #[test]
    fn compiler_emit_cache_retires_corrupt_records_and_driver_references() {
        let invocation = test_invocation();
        write_compiler_check_source(&invocation, &freestanding_source("!{}"));
        let plan = compiler_emit_plan(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        );
        execute_build_plan(&plan, &invocation).unwrap();

        let action_entry = only_nested_cache_entry(
            &invocation.cache_dir.join("actions/compiler-emits/v1"),
            "entry",
        );
        fs::write(&action_entry, b"corrupt").expect("corrupt compiler emit action entry");
        let corrupt_action = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&corrupt_action),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
        );
        assert_eq!(
            only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
            &ActionCacheOutcome::Hit
        );

        let link_entry =
            only_nested_cache_entry(&invocation.cache_dir.join("artifacts/links/v3"), "link");
        fs::write(&link_entry, b"corrupt").expect("corrupt Driver link entry");
        let corrupt_reference = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&corrupt_reference),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::Corrupt)
        );
        assert_eq!(
            only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
            &ActionCacheOutcome::Hit
        );

        let link_entry =
            only_nested_cache_entry(&invocation.cache_dir.join("artifacts/links/v3"), "link");
        fs::remove_file(link_entry).expect("remove Driver link entry");
        let missing_reference = execute_build_plan(&plan, &invocation).unwrap();
        assert_eq!(
            only_compiler_emit_outcome(&missing_reference),
            &ActionCacheOutcome::Miss(ActionCacheMissReason::NotFound)
        );
        assert_eq!(
            only_compiler_emit_outcome(&execute_build_plan(&plan, &invocation).unwrap()),
            &ActionCacheOutcome::Hit
        );
    }

    #[test]
    fn compiler_emit_cache_reuses_relocated_dynamic_source_closure() {
        let first = test_invocation();
        write_compiler_check_source(&first, &freestanding_source("!{}"));
        execute_build_plan(
            &compiler_emit_plan(
                &first,
                OptimizationMode::O2,
                Runtime::Freestanding,
                "bin/app",
            ),
            &first,
        )
        .unwrap();

        let mut relocated = test_invocation();
        relocated.cache_dir = first.cache_dir.clone();
        write_compiler_check_source(&relocated, &freestanding_source("!{}"));
        let report = execute_build_plan(
            &compiler_emit_plan(
                &relocated,
                OptimizationMode::O2,
                Runtime::Freestanding,
                "bin/app",
            ),
            &relocated,
        )
        .unwrap();

        assert_eq!(
            only_compiler_emit_outcome(&report),
            &ActionCacheOutcome::Hit
        );
        assert!(relocated.build_dir.join("bin/app").is_file());
    }

    #[test]
    fn compiler_emit_cache_does_not_publish_warnings() {
        let invocation = test_invocation();
        write_compiler_check_source(
            &invocation,
            &format!("using std::collections;\n{}", freestanding_source("!{}")),
        );
        let plan = compiler_emit_plan(
            &invocation,
            OptimizationMode::O2,
            Runtime::Freestanding,
            "bin/app",
        );

        for report in [
            execute_build_plan(&plan, &invocation).unwrap(),
            execute_build_plan(&plan, &invocation).unwrap(),
        ] {
            assert_eq!(
                only_compiler_emit_outcome(&report),
                &ActionCacheOutcome::Miss(ActionCacheMissReason::Uncacheable)
            );
        }
        assert!(
            !invocation
                .cache_dir
                .join("actions/compiler-emits/v1")
                .exists()
        );
    }

    #[test]
    fn generated_file_cache_read_failure_is_an_explicit_nonfatal_miss() {
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.cache_dir).unwrap();
        fs::write(invocation.cache_dir.join("actions"), b"occupied").unwrap();
        let plan = generated_plan(&invocation, "generated/source.nia", b"source");

        let report = execute_build_plan(&plan, &invocation).unwrap();

        assert_eq!(
            report.action_cache[0].outcome,
            ActionCacheOutcome::Miss(ActionCacheMissReason::ReadError)
        );
        assert_eq!(
            fs::read(invocation.build_dir.join("generated/source.nia")).unwrap(),
            b"source"
        );
    }

    #[test]
    fn failed_generated_file_publication_cleans_temporary_output() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("occupied");
        fs::create_dir_all(&output).unwrap();
        let plan = generated_plan(&invocation, "occupied", b"contents");

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(
            matches!(error, CoordinatorError::GeneratedFileIo { action, .. } if action.name() == "generate")
        );
        assert!(output.is_dir());
        assert_no_output_locks(&invocation);
        assert!(fs::read_dir(&invocation.build_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nia-generated-")
        }));
    }

    fn external_action() -> PlanAction {
        PlanAction {
            key: action("run"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                cache_policy: crate::CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("sh".to_string()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        }
    }

    fn staged_command_plan(invocation: &BuildInvocation, script: &str, output: &str) -> BuildPlan {
        staged_command_plan_outputs(invocation, script, &[output])
    }

    fn staged_command_plan_outputs(
        invocation: &BuildInvocation,
        script: &str,
        outputs: &[&str],
    ) -> BuildPlan {
        let outputs = outputs
            .iter()
            .map(|output| LogicalPath::new(LogicalPathRoot::Build, output).unwrap())
            .collect::<Vec<_>>();
        let mut arguments = vec![
            CommandArgument::Literal("-c".to_string()),
            CommandArgument::Literal(script.to_string()),
            CommandArgument::Literal("nia-build-tool".to_string()),
        ];
        arguments.extend(outputs.iter().cloned().map(CommandArgument::OutputPath));
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target_spec(invocation.toolchain.host_target()),
            artifact_target: target_spec(invocation.toolchain.artifact_target()),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("tool"),
                kind: ActionKind::ExternalCommand {
                    resource_class: ActionResourceClass::Conservative,
                    environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                    cache_policy: crate::CommandCachePolicy::Uncacheable,
                    program: CommandProgram::Search("sh".to_string()),
                    arguments,
                    working_directory: LogicalPath::new(
                        LogicalPathRoot::Package(PackageKey::root()),
                        "",
                    )
                    .unwrap(),
                    environment: Vec::new(),
                    inputs: Vec::new(),
                    outputs,
                },
            }],
            steps: vec![PlanStep {
                key: step("tool"),
                action: action("tool"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("tool")),
            selected_step: None,
        })
        .unwrap()
    }

    fn assert_no_staged_command_directories(parent: &Path) {
        assert!(fs::read_dir(parent).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nia-command-")
        }));
    }

    fn assert_no_output_transaction_journals(invocation: &BuildInvocation) {
        let root = invocation.build_dir.join(".nia-transactions/v1");
        if root.is_dir() {
            assert!(fs::read_dir(root).unwrap().next().is_none());
        }
    }

    fn prepare_test_staged_outputs(
        invocation: &BuildInvocation,
        destinations: &[PathBuf],
    ) -> Result<StagedOutputTransaction, CoordinatorError> {
        let logical = destinations
            .iter()
            .map(|destination| {
                let relative = destination.strip_prefix(&invocation.build_dir).unwrap();
                let path = relative
                    .components()
                    .map(|component| component.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");
                LogicalPath::new(LogicalPathRoot::Build, &path).unwrap()
            })
            .collect::<Vec<_>>();
        let resolved = logical
            .iter()
            .zip(destinations.iter().cloned())
            .collect::<Vec<_>>();
        prepare_staged_outputs(&external_action(), &invocation.build_dir, &resolved)
    }

    #[test]
    fn external_command_publishes_one_declared_output_atomically() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("tool/result.txt");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"old").unwrap();
        let plan = staged_command_plan(&invocation, "printf new > \"$1\"", "tool/result.txt");

        let report = execute_build_plan(&plan, &invocation).unwrap();

        assert_eq!(report.actions, [action("tool")]);
        assert_eq!(fs::read(&output).unwrap(), b"new");
        assert_no_staged_command_directories(output.parent().unwrap());
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn external_command_publishes_multiple_outputs_as_one_transaction() {
        let invocation = test_invocation();
        let first = invocation.build_dir.join("tool/first.txt");
        let second = invocation.build_dir.join("other/second.txt");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"old first").unwrap();
        fs::write(&second, b"old second").unwrap();
        let plan = staged_command_plan_outputs(
            &invocation,
            "printf 'new first' > \"$1\"; printf 'new second' > \"$2\"",
            &["tool/first.txt", "other/second.txt"],
        );

        let report = execute_build_plan(&plan, &invocation).unwrap();

        assert_eq!(report.actions, [action("tool")]);
        assert_eq!(fs::read(&first).unwrap(), b"new first");
        assert_eq!(fs::read(&second).unwrap(), b"new second");
        assert_no_staged_command_directories(second.parent().unwrap());
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn missing_transaction_output_preserves_every_previous_output() {
        let invocation = test_invocation();
        let first = invocation.build_dir.join("first.txt");
        let second = invocation.build_dir.join("second.txt");
        fs::create_dir_all(&invocation.build_dir).unwrap();
        fs::write(&first, b"accepted first").unwrap();
        fs::write(&second, b"accepted second").unwrap();
        let plan = staged_command_plan_outputs(
            &invocation,
            "printf 'rejected first' > \"$1\"",
            &["first.txt", "second.txt"],
        );

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::StagedOutput {
                operation: "inspect command-produced",
                ..
            }
        ));
        assert_eq!(fs::read(&first).unwrap(), b"accepted first");
        assert_eq!(fs::read(&second).unwrap(), b"accepted second");
        assert_no_staged_command_directories(&invocation.build_dir);
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn partial_transaction_commit_restores_old_and_absent_destinations() {
        let invocation = test_invocation();
        let first = invocation.build_dir.join("first.txt");
        let absent = invocation.build_dir.join("absent.txt");
        let last = invocation.build_dir.join("last.txt");
        fs::create_dir_all(&invocation.build_dir).unwrap();
        fs::write(&first, b"old first").unwrap();
        fs::write(&last, b"old last").unwrap();
        let staged = prepare_test_staged_outputs(
            &invocation,
            &[first.clone(), absent.clone(), last.clone()],
        )
        .unwrap();
        fs::write(&staged.outputs[0].temporary, b"new first").unwrap();
        fs::write(&staged.outputs[1].temporary, b"new absent").unwrap();
        fs::write(&staged.outputs[2].temporary, b"new last").unwrap();

        let error = publish_staged_outputs_with(&external_action(), staged, |index| {
            if index == 2 {
                Err(io::Error::other("injected transaction commit failure"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::StagedOutput {
                operation: "commit transaction entry for",
                ..
            }
        ));
        assert_eq!(fs::read(&first).unwrap(), b"old first");
        assert!(!absent.exists());
        assert_eq!(fs::read(&last).unwrap(), b"old last");
        assert_no_staged_command_directories(&invocation.build_dir);
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn coordinator_recovers_interrupted_transaction_before_dispatch() {
        let invocation = test_invocation();
        let previous = invocation.build_dir.join("tool/previous.txt");
        let absent = invocation.build_dir.join("other/absent.txt");
        fs::create_dir_all(previous.parent().unwrap()).unwrap();
        fs::create_dir_all(absent.parent().unwrap()).unwrap();
        fs::write(&previous, b"old").unwrap();
        let staged =
            prepare_test_staged_outputs(&invocation, &[previous.clone(), absent.clone()]).unwrap();
        fs::write(&staged.outputs[0].temporary, b"new old").unwrap();
        fs::write(&staged.outputs[1].temporary, b"new absent").unwrap();
        staged.journal.mark_prepared(&[true, false]).unwrap();
        fs::rename(&previous, &staged.outputs[0].backup).unwrap();
        fs::rename(&staged.outputs[0].temporary, &previous).unwrap();
        fs::rename(&staged.outputs[1].temporary, &absent).unwrap();
        let plan = generated_plan(&invocation, "after-recovery.txt", b"continued");

        execute_build_plan(&plan, &invocation).unwrap();

        assert_eq!(fs::read(previous).unwrap(), b"old");
        assert!(!absent.exists());
        assert_eq!(
            fs::read(invocation.build_dir.join("after-recovery.txt")).unwrap(),
            b"continued"
        );
        assert_no_staged_command_directories(&invocation.build_dir.join("tool"));
    }

    #[test]
    fn failed_transaction_acceptance_restores_every_destination() {
        let invocation = test_invocation();
        let first = invocation.build_dir.join("first.txt");
        let second = invocation.build_dir.join("second.txt");
        fs::create_dir_all(&invocation.build_dir).unwrap();
        fs::write(&first, b"old first").unwrap();
        fs::write(&second, b"old second").unwrap();
        let staged =
            prepare_test_staged_outputs(&invocation, &[first.clone(), second.clone()]).unwrap();
        fs::write(&staged.outputs[0].temporary, b"new first").unwrap();
        fs::write(&staged.outputs[1].temporary, b"new second").unwrap();
        let committed_directory = staged.committed_directory.clone();

        let error = publish_staged_outputs_with(&external_action(), staged, |index| {
            if index == 1 {
                fs::create_dir(&committed_directory)?;
                fs::write(committed_directory.join("occupied"), b"occupied")?;
            }
            Ok(())
        })
        .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::StagedOutput {
                operation: "mark transaction committed at",
                ..
            }
        ));
        assert_eq!(fs::read(&first).unwrap(), b"old first");
        assert_eq!(fs::read(&second).unwrap(), b"old second");
        fs::remove_dir_all(committed_directory).unwrap();
        assert_no_staged_command_directories(&invocation.build_dir);
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn failed_external_command_retires_staging_and_preserves_old_output() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("tool/result.txt");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"accepted").unwrap();
        let plan = staged_command_plan(
            &invocation,
            "printf rejected > \"$1\"; exit 9",
            "tool/result.txt",
        );

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::ExternalCommand(details)
                if matches!(details.failure, ExternalCommandFailure::Exit { status, .. }
                    if status.code() == Some(9))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"accepted");
        assert_no_staged_command_directories(output.parent().unwrap());
        assert_no_output_transaction_journals(&invocation);
    }

    #[test]
    fn missing_external_output_is_typed_and_retires_staging() {
        let invocation = test_invocation();
        let output = invocation.build_dir.join("tool/result.txt");
        let plan = staged_command_plan(&invocation, "true", "tool/result.txt");

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::StagedOutput {
                action,
                operation: "inspect command-produced",
                ..
            } if action.name() == "tool"
        ));
        assert!(!output.exists());
        assert_no_staged_command_directories(output.parent().unwrap());
        assert_no_output_transaction_journals(&invocation);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_staged_argument_failure_retires_staging() {
        use std::os::unix::ffi::OsStringExt as _;

        let mut invocation = test_invocation();
        invocation.build_dir = invocation
            .package_root
            .join(std::ffi::OsString::from_vec(b"build-\xff".to_vec()));
        let output = invocation.build_dir.join("tool/result.txt");
        let plan = staged_command_plan(&invocation, "printf new > \"$1\"", "tool/result.txt");

        let error = execute_build_plan(&plan, &invocation).unwrap_err();

        assert!(matches!(error, CoordinatorError::NonUtf8Path { .. }));
        assert!(!output.exists());
        assert_no_staged_command_directories(output.parent().unwrap());
    }

    fn execute_test_command(
        action: &PlanAction,
        arguments: &[String],
        working_directory: &Path,
        timeout: Duration,
    ) -> Result<(), CoordinatorError> {
        execute_external_command(
            action,
            ResolvedExternalCommand {
                program: "sh",
                arguments,
                working_directory,
                environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                environment: &[],
            },
            ExternalExecutionPolicy {
                timeout,
                forward_output: false,
                cancellation: None,
            },
        )
    }

    #[test]
    fn external_command_failure_retains_status_and_bounded_output_tails() {
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.package_root).unwrap();
        let action = external_action();
        let arguments = vec![
            "-c".to_string(),
            "printf 'command stdout'; printf 'command stderr' >&2; exit 7".to_string(),
        ];

        let error = execute_test_command(
            &action,
            &arguments,
            &invocation.package_root,
            Duration::from_secs(5),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::ExternalCommand(details)
                if details.action.name() == "run"
                    && details.arguments == arguments
                    && matches!(details.failure, ExternalCommandFailure::Exit {
                        status,
                        ref stdout,
                        ref stderr,
                    } if status.code() == Some(7)
                        && stdout == b"command stdout"
                        && stderr == b"command stderr")
        ));
    }

    #[test]
    fn external_command_clear_environment_keeps_only_declared_values() {
        assert!(std::env::var_os("HOME").is_some());
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.package_root).unwrap();
        let action = external_action();
        let arguments = vec![
            "-c".to_string(),
            "test -z \"${HOME+x}\" && test \"$MODE\" = explicit".to_string(),
        ];
        let environment = [crate::EnvironmentInput {
            name: "MODE".to_string(),
            value: Some("explicit".to_string()),
        }];

        execute_external_command(
            &action,
            ResolvedExternalCommand {
                program: "sh",
                arguments: &arguments,
                working_directory: &invocation.package_root,
                environment_policy: crate::CommandEnvironmentPolicy::Clear,
                environment: &environment,
            },
            ExternalExecutionPolicy {
                timeout: Duration::from_secs(5),
                forward_output: false,
                cancellation: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn external_output_tail_discards_only_the_oldest_bytes() {
        let mut tail = vec![b'a'; EXTERNAL_OUTPUT_TAIL_BYTES - 2];
        append_output_tail(&mut tail, b"bcdef");

        assert_eq!(tail.len(), EXTERNAL_OUTPUT_TAIL_BYTES);
        assert_eq!(&tail[..3], b"aaa");
        assert_eq!(&tail[tail.len() - 5..], b"bcdef");
    }

    #[test]
    fn external_command_timeout_terminates_owned_process_group() {
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.package_root).unwrap();
        let action = external_action();
        let started = Instant::now();

        let error = execute_test_command(
            &action,
            &["-c".to_string(), "sleep 30".to_string()],
            &invocation.package_root,
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::ExternalCommand(details)
                if matches!(details.failure, ExternalCommandFailure::TimedOut { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn external_command_cancellation_terminates_owned_process_group() {
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.package_root).unwrap();
        let marker = invocation.package_root.join("command-started");
        let earliest_failure = Arc::new(AtomicUsize::new(usize::MAX));
        let cancellation = ActionCancellation {
            earliest_failure: Arc::clone(&earliest_failure),
            position: 1,
        };
        let working_directory = invocation.package_root.clone();
        let action = external_action();
        let worker = std::thread::spawn(move || {
            let arguments = vec![
                "-c".to_string(),
                "touch \"$1\"; sleep 30".to_string(),
                "nia-build-test".to_string(),
                marker.to_string_lossy().into_owned(),
            ];
            execute_external_command(
                &action,
                ResolvedExternalCommand {
                    program: "sh",
                    arguments: &arguments,
                    working_directory: &working_directory,
                    environment_policy: crate::CommandEnvironmentPolicy::Inherit,
                    environment: &[],
                },
                ExternalExecutionPolicy {
                    timeout: Duration::from_secs(30),
                    forward_output: false,
                    cancellation: Some(&cancellation),
                },
            )
        });
        let started = Instant::now();
        let marker = invocation.package_root.join("command-started");
        while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let command_started = marker.exists();
        earliest_failure.store(0, Ordering::Release);

        let error = worker.join().expect("external command worker").unwrap_err();

        assert!(command_started);
        assert!(matches!(
            error,
            CoordinatorError::ExternalCommand(details)
                if matches!(details.failure, ExternalCommandFailure::Cancelled { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn external_command_success_retires_background_descendants_holding_pipes() {
        let invocation = test_invocation();
        fs::create_dir_all(&invocation.package_root).unwrap();
        let action = external_action();
        let started = Instant::now();

        execute_test_command(
            &action,
            &["-c".to_string(), "sleep 30 & exit 0".to_string()],
            &invocation.package_root,
            Duration::from_secs(5),
        )
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
