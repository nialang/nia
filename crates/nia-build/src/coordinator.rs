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
use nia_query::QuerySession;
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
        ExternalCommandCacheIdentity, ExternalCommandCacheLookup, ExternalCommandContentIdentity,
        GeneratedFileCache, GeneratedFileCacheIdentity, GeneratedFileCacheLookup,
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
    InstallArtifactIo {
        action: ActionKey,
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
    },
    StaticArchiveLinkInputIo {
        action: ActionKey,
        path: PathBuf,
        operation: &'static str,
        error: io::Error,
    },
    ExternalCommandIo {
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
