// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, LinkExecutableRequest, ModuleMap,
    NiaOptimizationLevel, Runtime as DriverRuntime, SourcePath,
};
use nia_query::QueryFingerprintBuilder;
use nia_target_config::TargetConfig;

use crate::{
    ActionKey, ActionKind, ArtifactKey, BuildInvocation, BuildPlan, CommandArgument,
    CommandProgram, LogicalPath, LogicalPathRoot, ModuleKey, OptimizationMode, PackageKey,
    PlanAction, PlanArtifact, PlanModule, Runtime, StepKey, TargetSpec, lock::ScopedFileLock,
};

const EXTERNAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(7 * 60);
const EXTERNAL_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
const EXTERNAL_WAIT_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub steps: Vec<StepKey>,
    pub actions: Vec<ActionKey>,
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
    Exit {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

#[derive(Debug)]
pub enum CoordinatorError {
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
    let mut executor = DriverActionExecutor::new(plan, invocation);
    execute_selected_closure(plan, |action| executor.execute(action))
}

fn execute_selected_closure(
    plan: &BuildPlan,
    mut execute: impl FnMut(&PlanAction) -> Result<(), CoordinatorError>,
) -> Result<ExecutionReport, CoordinatorError> {
    let Some(selected) = plan.selected_step().or_else(|| plan.default_step()) else {
        return Ok(ExecutionReport {
            steps: Vec::new(),
            actions: Vec::new(),
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
    };

    while let Some(index) = ready.pop_first() {
        let step = &steps[index];
        if executed_actions.insert(step.action.clone()) {
            let action = find_action(plan.actions(), &step.action).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", step.key.name()),
                    format!("action `{}`", step.action.name()),
                )
            })?;
            execute(action)?;
            report.actions.push(action.key.clone());
        }
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

    if report.steps.len() != closure.len() {
        return Err(inconsistent(
            "selected step closure",
            "acyclic dependency order".to_string(),
        ));
    }
    Ok(report)
}

struct DriverActionExecutor<'a> {
    plan: &'a BuildPlan,
    invocation: &'a BuildInvocation,
    drivers: BTreeMap<TargetSpec, Driver>,
}

impl<'a> DriverActionExecutor<'a> {
    fn new(plan: &'a BuildPlan, invocation: &'a BuildInvocation) -> Self {
        Self {
            plan,
            invocation,
            drivers: BTreeMap::new(),
        }
    }

    fn execute(&mut self, action: &PlanAction) -> Result<(), CoordinatorError> {
        let _output_locks = self.acquire_output_locks(action)?;
        self.execute_with_output_ownership(action)
    }

    fn execute_with_output_ownership(
        &mut self,
        action: &PlanAction,
    ) -> Result<(), CoordinatorError> {
        match &action.kind {
            ActionKind::Aggregate => Ok(()),
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
            } => {
                let request = self.check_request(action, module, *runtime)?;
                let driver = self.driver(target);
                driver
                    .check_entry(request)
                    .result
                    .map(|_| ())
                    .map_err(|error| CoordinatorError::Driver {
                        action: action.key.clone(),
                        error: Box::new(error),
                    })
            }
            ActionKind::CompilerEmit { artifact, target } => {
                let artifact = self.artifact(action, artifact)?;
                let request =
                    self.check_request(action, &artifact.root_module, artifact.runtime)?;
                let output = self.resolve_path(action, &artifact.output)?;
                let driver = self.driver(target);
                driver
                    .link_executable(LinkExecutableRequest::new(request, output))
                    .result
                    .map(|_| ())
                    .map_err(|error| CoordinatorError::Driver {
                        action: action.key.clone(),
                        error: Box::new(error),
                    })
            }
            ActionKind::ExternalCommand {
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
                let resolved_output = outputs
                    .first()
                    .map(|output| self.resolve_path(action, output).map(|path| (output, path)))
                    .transpose()?;
                let mut staged = resolved_output
                    .as_ref()
                    .map(|(_, output)| prepare_staged_output(action, output))
                    .transpose()?;
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
                            let Some(((output, _), staged)) =
                                resolved_output.as_ref().zip(staged.as_ref())
                            else {
                                return Err(inconsistent(
                                    format!("action `{}`", action.key.name()),
                                    "declared command output binding".to_string(),
                                ));
                            };
                            if *output != path {
                                return Err(inconsistent(
                                    format!("action `{}`", action.key.name()),
                                    "matching command output binding".to_string(),
                                ));
                            }
                            path_text(action, &staged.temporary)
                        }
                    })
                    .collect::<Result<Vec<_>, CoordinatorError>>();
                let resolved_arguments = match resolved_arguments {
                    Ok(arguments) => arguments,
                    Err(cause) => {
                        return match staged.take() {
                            Some(staged) => {
                                cleanup_staged_output(action, staged, Some(Box::new(cause)))
                            }
                            None => Err(cause),
                        };
                    }
                };
                let execution = execute_external_command(
                    action,
                    ResolvedExternalCommand {
                        program: &program,
                        arguments: &resolved_arguments,
                        working_directory: &working_directory,
                        environment,
                    },
                    ExternalExecutionPolicy {
                        timeout: EXTERNAL_COMMAND_TIMEOUT,
                        forward_output: true,
                    },
                );
                match (execution, staged) {
                    (Ok(()), Some(staged)) => publish_staged_output(action, staged),
                    (Ok(()), None) => Ok(()),
                    (Err(cause), Some(staged)) => {
                        cleanup_staged_output(action, staged, Some(Box::new(cause)))
                    }
                    (Err(cause), None) => Err(cause),
                }
            }
            ActionKind::GeneratedFile { output, contents } => {
                let output = self.resolve_path(action, output)?;
                write_generated_file(action, &output, contents)
            }
            ActionKind::Uncacheable { .. } => Err(unsupported(action, "uncacheable")),
        }
    }

    fn acquire_output_locks(
        &self,
        action: &PlanAction,
    ) -> Result<Vec<ScopedFileLock>, CoordinatorError> {
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
        outputs
            .into_iter()
            .map(|output| {
                let resolved = self.resolve_path(action, output)?;
                let lock = output_lock_path(&self.invocation.cache_dir, output);
                ScopedFileLock::acquire(lock.clone()).map_err(|error| {
                    CoordinatorError::AcquireOutputLock {
                        action: action.key.clone(),
                        output: resolved,
                        lock,
                        error,
                    }
                })
            })
            .collect()
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
        let entry = self.resolve_source_text(action, &module.root_source)?;
        let mut module_map = ModuleMap::new();
        for import in &module.imports {
            let path = SourcePath::new(self.resolve_source_text(action, &import.path)?);
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
        Ok(CheckRequest::new(entry)
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

    fn driver(&mut self, target: &TargetSpec) -> &Driver {
        self.drivers.entry(target.clone()).or_insert_with(|| {
            Driver::with_config(
                DriverConfig {
                    artifact_cache_dir: Some(self.invocation.cache_dir.clone()),
                    ..DriverConfig::new(Arc::clone(&self.invocation.toolchain))
                }
                .with_artifact_target(target_config(target)),
            )
        })
    }

    fn resolve_source_text(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<String, CoordinatorError> {
        let path = self.resolve_path(action, logical)?;
        let text = path.to_str().ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.clone(),
        })?;
        Ok(text.to_string())
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
    environment: &'a [crate::EnvironmentInput],
}

#[derive(Clone, Copy)]
struct ExternalExecutionPolicy {
    timeout: Duration,
    forward_output: bool,
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
    let status = loop {
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

fn output_lock_path(cache_dir: &Path, output: &LogicalPath) -> PathBuf {
    let mut builder = QueryFingerprintBuilder::new("nia.build.output-lock.v1");
    builder.write_str(&output.protocol_path());
    let [first, second] = builder.finish().parts();
    cache_dir
        .join("coordination/output-locks")
        .join(format!("{first:016x}{second:016x}.lock"))
}

static STAGED_OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct StagedOutput {
    destination: PathBuf,
    directory: PathBuf,
    temporary: PathBuf,
}

fn prepare_staged_output(
    action: &PlanAction,
    destination: &Path,
) -> Result<StagedOutput, CoordinatorError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            staged_output_io(
                action,
                destination,
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
        match fs::create_dir(&directory) {
            Ok(()) => {
                return Ok(StagedOutput {
                    destination: destination.to_path_buf(),
                    temporary: directory.join("output"),
                    directory,
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

fn publish_staged_output(
    action: &PlanAction,
    staged: StagedOutput,
) -> Result<(), CoordinatorError> {
    let result = (|| {
        let metadata = fs::symlink_metadata(&staged.temporary).map_err(|error| {
            staged_output_io(
                action,
                &staged.temporary,
                "inspect command-produced",
                error,
                None,
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(staged_output_io(
                action,
                &staged.temporary,
                "publish non-file",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "external command output must be a regular file",
                ),
                None,
            ));
        }
        fs::File::open(&staged.temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                staged_output_io(
                    action,
                    &staged.temporary,
                    "sync command-produced",
                    error,
                    None,
                )
            })?;
        fs::rename(&staged.temporary, &staged.destination).map_err(|error| {
            staged_output_io(action, &staged.destination, "publish", error, None)
        })?;
        let parent = staged.destination.parent().ok_or_else(|| {
            staged_output_io(
                action,
                &staged.destination,
                "resolve parent for",
                io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"),
                None,
            )
        })?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                staged_output_io(action, parent, "sync parent directory for", error, None)
            })
    })();
    match result {
        Ok(()) => fs::remove_dir(&staged.directory).map_err(|error| {
            staged_output_io(
                action,
                &staged.directory,
                "retire staging directory for",
                error,
                None,
            )
        }),
        Err(cause) => cleanup_staged_output(action, staged, Some(Box::new(cause))),
    }
}

fn cleanup_staged_output(
    action: &PlanAction,
    staged: StagedOutput,
    cause: Option<Box<CoordinatorError>>,
) -> Result<(), CoordinatorError> {
    match fs::remove_dir_all(&staged.directory) {
        Ok(()) => match cause {
            Some(cause) => Err(*cause),
            None => Ok(()),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => match cause {
            Some(cause) => Err(*cause),
            None => Ok(()),
        },
        Err(error) => Err(staged_output_io(
            action,
            &staged.directory,
            "clean up",
            error,
            cause,
        )),
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
    use crate::{BuildPlanDraft, PlanAction, PlanPackage, PlanStep};
    use std::sync::{Arc, OnceLock};

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
        let sequence = GENERATED_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let package_root = std::env::temp_dir().join(format!(
            "nia-build-coordinator-test-{}-{sequence}",
            std::process::id()
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

        let report = execute_selected_closure(&plan, |item| {
            observed.push(item.key.name().to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(observed, ["shared", "left", "right", "final"]);
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

        let report = execute_selected_closure(&plan, |item| {
            observed.push(item.key.name().to_string());
            Ok(())
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

        let error = execute_selected_closure(&plan, |item| Err(unsupported(item, "test-failure")))
            .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "test-failure" }
                if action.name() == "first"
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
        fs::create_dir_all(&invocation.cache_dir).unwrap();
        fs::write(invocation.cache_dir.join("coordination"), b"occupied").unwrap();
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
        let output = LogicalPath::new(LogicalPathRoot::Build, output).unwrap();
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
                    program: CommandProgram::Search("sh".to_string()),
                    arguments: vec![
                        CommandArgument::Literal("-c".to_string()),
                        CommandArgument::Literal(script.to_string()),
                        CommandArgument::Literal("nia-build-tool".to_string()),
                        CommandArgument::OutputPath(output.clone()),
                    ],
                    working_directory: LogicalPath::new(
                        LogicalPathRoot::Package(PackageKey::root()),
                        "",
                    )
                    .unwrap(),
                    environment: Vec::new(),
                    inputs: Vec::new(),
                    outputs: vec![output],
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
                environment: &[],
            },
            ExternalExecutionPolicy {
                timeout,
                forward_output: false,
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
