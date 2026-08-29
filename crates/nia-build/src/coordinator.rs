// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic selected-closure scheduling and typed action execution.
//!
//! Ready actions run in canonical waves through the inherited `QuerySession`
//! budget. Completion order is not observable, failures cancel later canonical
//! work, and every output-producing action crosses the locked journaled
//! publication boundary before its result becomes visible.

mod executor;
mod external_command;
mod helpers;
mod publication;
mod scheduling;

use executor::DriverActionExecutor;
use external_command::*;
use helpers::*;
use publication::*;
use scheduling::ActionCancellation;
pub use scheduling::execute_build_plan;
#[cfg(test)]
use scheduling::{
    ActionOutcome, action_resource_capacity, execute_selected_closure, run_action_tasks,
};

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
    CheckRequest, Driver, DriverConfig, DriverError, EmitObjectRequest, ExecutableCacheRestore,
    LinkExecutableRequest, ModuleMap, NiaOptimizationLevel, ObjectOutput, Runtime as DriverRuntime,
    SourcePath,
};
use nia_linker::{ArchiveOptions, LinkOptions, StaticArchiveLinkInput};
use nia_query::{QueryFingerprintBytesWriter, QuerySession};
use nia_target_config::TargetConfig;

use crate::{
    ActionCacheMissReason, ActionCacheOutcome, ActionCacheReport, ActionKey, ActionKind,
    ActionResourceClass, ArtifactKey, BuildInvocation, BuildPlan, CommandArgument,
    CommandCachePolicy, CommandEnvironmentPolicy, CommandProgram, EnvironmentInput, LogicalPath,
    LogicalPathRoot, ModuleKey, OptimizationMode, OutputRecoveryError, PackageKey, PlanAction,
    PlanArtifact, PlanArtifactKind, PlanModule, Runtime, StepKey, TargetSpec,
    action_cache::{
        CompilerCheckCache, CompilerCheckCacheIdentity, CompilerCheckCacheLookup,
        CompilerEmitCache, CompilerEmitCacheIdentity, CompilerEmitCacheIdentityInput,
        CompilerEmitCacheLinkInput, CompilerEmitCacheLookup, ExternalCommandCache,
        ExternalCommandCacheHit, ExternalCommandCacheIdentity, ExternalCommandCacheLookup,
        ExternalCommandCacheOutput, ExternalCommandContentIdentity, GeneratedFileCache,
        GeneratedFileCacheIdentity, GeneratedFileCacheLookup,
    },
    lock::{ProcessIdentity, ScopedFileLock, output_lock_path},
    output_recovery::{
        OutputTransactionJournal, TransactionOutput, TransactionOutputKind,
        recover_interrupted_output_transactions,
    },
    process_output::{
        CapturedStream, StreamCapture, capture_stream, prepare_process_group,
        terminate_process_descendants, terminate_process_tree,
    },
    resources::ActionResourceBudget,
};

const EXTERNAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(7 * 60);
const EXTERNAL_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
const EXTERNAL_WAIT_POLL: Duration = Duration::from_millis(10);

/// Deterministic visible result of executing a selected plan closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    /// Steps selected and completed in canonical order.
    pub steps: Vec<StepKey>,
    /// Actions selected and completed in canonical order.
    pub actions: Vec<ActionKey>,
    /// Per-action cache outcomes in action order.
    pub action_cache: Vec<ActionCacheReport>,
}

/// Difference between a plan target and the current invocation target.
#[derive(Debug)]
pub struct TargetMismatch {
    /// Target role, such as `host` or `artifact`.
    pub role: &'static str,
    /// Target encoded in the frozen plan.
    pub expected: TargetSpec,
    /// Target supplied by the current toolchain invocation.
    pub found: TargetSpec,
}

/// Details for a module import that cannot be resolved in the plan closure.
#[derive(Debug)]
pub struct InvalidModuleImport {
    /// Action requesting the import.
    pub action: ActionKey,
    /// Module containing the import.
    pub module: ModuleKey,
    /// Source-level import name.
    pub name: String,
    /// Stable explanation of the failed resolution.
    pub reason: String,
}

/// Structured failure from an external command action.
#[derive(Debug)]
pub struct ExternalCommandError {
    /// Action that spawned the command.
    pub action: ActionKey,
    /// Resolved program displayed to the user.
    pub program: String,
    /// Display-safe command arguments.
    pub arguments: Vec<String>,
    /// Physical working directory used for the process.
    pub working_directory: PathBuf,
    /// Spawn, capture, timeout, cancellation, or exit detail.
    pub failure: ExternalCommandFailure,
}

/// Process and output failure detail retained by an external command error.
#[derive(Debug)]
pub enum ExternalCommandFailure {
    /// The operating system rejected process creation.
    Spawn {
        /// Underlying spawn error.
        error: io::Error,
    },
    /// A configured standard stream was unavailable after spawning.
    MissingPipe {
        /// Missing stream name.
        stream: &'static str,
    },
    /// Waiting for the child process failed.
    Wait {
        /// Underlying wait error.
        error: io::Error,
    },
    /// A stream-capture worker panicked before returning its result.
    CaptureThread {
        /// Stream owned by the failed worker.
        stream: &'static str,
    },
    /// A stream-capture worker could not be spawned.
    CaptureWorkerSpawn {
        /// Stream assigned to the worker.
        stream: &'static str,
        /// Underlying thread-spawn error.
        error: io::Error,
    },
    /// Reading a captured stream failed.
    StreamIo {
        /// Stream that could not be read.
        stream: &'static str,
        /// Underlying I/O error.
        error: io::Error,
    },
    /// The command exceeded its configured execution timeout.
    TimedOut {
        /// Timeout applied to the command.
        timeout: Duration,
        /// Bounded captured standard-output tail.
        stdout: Vec<u8>,
        /// Bounded captured standard-error tail.
        stderr: Vec<u8>,
    },
    /// The command was terminated after build cancellation.
    Cancelled {
        /// Bounded captured standard-output tail.
        stdout: Vec<u8>,
        /// Bounded captured standard-error tail.
        stderr: Vec<u8>,
    },
    /// The command exited unsuccessfully.
    Exit {
        /// Child exit status.
        status: ExitStatus,
        /// Bounded captured standard-output tail.
        stdout: Vec<u8>,
        /// Bounded captured standard-error tail.
        stderr: Vec<u8>,
    },
}

/// Typed failure while validating, scheduling, executing, or publishing a plan.
#[derive(Debug)]
pub enum CoordinatorError {
    /// An action was cancelled after another action failed.
    Cancelled {
        /// Cancelled action.
        action: ActionKey,
    },
    /// Frozen-plan and invocation targets differ.
    TargetMismatch(Box<TargetMismatch>),
    /// A frozen plan contains an unresolved internal reference.
    InconsistentPlan {
        /// Declaration owning the reference.
        owner: String,
        /// Description of the missing declaration.
        missing: String,
    },
    /// An action references a package without a physical root mapping.
    UnmappedPackage {
        /// Action containing the reference.
        action: ActionKey,
        /// Package without a mapping.
        package: PackageKey,
    },
    /// A module import cannot be resolved in the executable plan closure.
    InvalidModuleImport(Box<InvalidModuleImport>),
    /// A resolved physical path cannot be represented as UTF-8.
    NonUtf8Path {
        /// Action using the path.
        action: ActionKey,
        /// Rejected physical path.
        path: PathBuf,
    },
    /// Generated-file staging or publication failed.
    GeneratedFileIo {
        /// Action materializing the file.
        action: ActionKey,
        /// Physical path involved in the failure.
        path: PathBuf,
        /// Operation attempted on the path.
        operation: &'static str,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Artifact installation staging or publication failed.
    InstallArtifactIo {
        /// Action installing the artifact.
        action: ActionKey,
        /// Physical path involved in the failure.
        path: PathBuf,
        /// Operation attempted on the path.
        operation: &'static str,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Reading or staging a static archive link input failed.
    StaticArchiveLinkInputIo {
        /// Compiler action consuming the archive.
        action: ActionKey,
        /// Physical archive path.
        path: PathBuf,
        /// Operation attempted on the path.
        operation: &'static str,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Preparing an external command input, output, or environment failed.
    ExternalCommandIo {
        /// External-command action.
        action: ActionKey,
        /// Physical path involved in the failure.
        path: PathBuf,
        /// Operation attempted on the path.
        operation: &'static str,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// A spawned external command failed.
    ExternalCommand(Box<ExternalCommandError>),
    /// Staging or retiring an output failed.
    StagedOutput {
        /// Action owning the staged output.
        action: ActionKey,
        /// Physical staging path.
        path: PathBuf,
        /// Operation attempted on the path.
        operation: &'static str,
        /// Underlying filesystem error.
        error: io::Error,
        /// Original action failure retained during cleanup, when present.
        cause: Option<Box<CoordinatorError>>,
    },
    /// Acquiring exclusive ownership of an output failed.
    AcquireOutputLock {
        /// Action requesting ownership.
        action: ActionKey,
        /// Physical output path.
        output: PathBuf,
        /// Lock path coordinating the output.
        lock: PathBuf,
        /// Underlying lock error.
        error: io::Error,
    },
    /// Recovery of an interrupted output transaction failed.
    OutputRecovery(Box<OutputRecoveryError>),
    /// The coordinator has no executor for an action kind.
    UnsupportedAction {
        /// Unsupported action.
        action: ActionKey,
        /// Stable action-kind name.
        kind: &'static str,
    },
    /// Compiler-driver execution failed for an action.
    Driver {
        /// Compiler action that failed.
        action: ActionKey,
        /// Underlying compiler-driver error.
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
            Self::InstallArtifactIo {
                action,
                path,
                operation,
                error,
            } => write!(
                f,
                "build action `{}` failed to {operation} installed artifact `{}`: {error}",
                action.name(),
                path.display()
            ),
            Self::StaticArchiveLinkInputIo {
                action,
                path,
                operation,
                error,
            } => write!(
                f,
                "build action `{}` failed to {operation} static archive link input `{}`: {error}",
                action.name(),
                path.display()
            ),
            Self::ExternalCommandIo {
                action,
                path,
                operation,
                error,
            } => write!(
                f,
                "build action `{}` failed to {operation} external command input `{}`: {error}",
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

#[cfg(test)]
mod tests;
