// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured reports for build bootstrap and coordinator failures.
//!
//! The build engine keeps `BuildError` and `CoordinatorError` rich so the
//! scheduler can make decisions. This module is the only presentation bridge
//! for those errors: it assigns stable diagnostic codes, bounds child output,
//! and keeps the generated runner from becoming the apparent source of a
//! failure in `build.nia`.

use std::{fmt::Write as _, path::Path};

use nia_diagnostic::{
    Diagnostic, DiagnosticReportConfig, build_diagnostic_report, codes, render_diagnostic,
    render_diagnostics_json,
};

use crate::{
    BuildError, CoordinatorError, ExternalCommandError, ExternalCommandFailure, TestFailure,
};

/// Renders one build failure using the canonical diagnostic layout.
pub fn render_build_error(
    error: &BuildError,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    if let BuildError::CompileRunner { path, error, .. } = error {
        // Driver diagnostics already contain the source-owned build.nia
        // locations. Do not wrap them in a synthetic runner diagnostic.
        return nia_driver::render_driver_error(error, Some(path), primary_source);
    }
    if let BuildError::ExecuteBuildPlan { error } = error {
        if let CoordinatorError::Driver { error, .. } = error.as_ref() {
            // Action diagnostics carry their own source paths. Preserve that
            // ownership instead of rendering them as a generic build error.
            return nia_driver::render_driver_error(error, primary_path, primary_source);
        }
    }

    let diagnostics = build_error_diagnostics(error);
    render_diagnostic_list(
        "build diagnostics:",
        &diagnostics,
        primary_path,
        primary_source,
    )
}

/// Renders a build failure as deterministic machine-readable JSON.
pub fn render_build_error_json(error: &BuildError) -> String {
    if let BuildError::CompileRunner { error, .. } = error {
        return nia_driver::render_driver_error_json(error);
    }
    if let BuildError::ExecuteBuildPlan { error } = error {
        if let CoordinatorError::Driver { error, .. } = error.as_ref() {
            return nia_driver::render_driver_error_json(error);
        }
    }
    let diagnostics = build_error_diagnostics(error);
    render_diagnostics_json(&diagnostics, DiagnosticReportConfig::default())
}

fn render_diagnostic_list(
    title: &str,
    diagnostics: &[Diagnostic],
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let report = build_diagnostic_report(diagnostics, DiagnosticReportConfig::default());
    let path = primary_path.unwrap_or("<build>");
    let source = primary_source.unwrap_or("");
    let mut output = String::new();
    output.push_str(title);
    output.push('\n');
    for diagnostic in report.entries() {
        output.push_str(&render_diagnostic(path, source, diagnostic));
        output.push('\n');
    }
    if report.suppressed_duplicates() > 0 || report.suppressed_by_limit() > 0 {
        let _ = writeln!(
            output,
            "note: suppressed {} diagnostic(s)",
            report.suppressed_duplicates() + report.suppressed_by_limit()
        );
    }
    output
}

fn build_error_diagnostics(error: &BuildError) -> Vec<Diagnostic> {
    match error {
        BuildError::ExecuteBuildPlan { error } => coordinator_diagnostics(error),
        BuildError::RunnerFailed {
            path,
            status,
            stdout,
            stderr,
        } => vec![runner_failure(path, *status, stdout, stderr)],
        BuildError::RunRunner { path, error } => vec![
            Diagnostic::user_error(
                codes::BUILD_RUNNER,
                "could not start the generated build runner",
            )
            .note(format!("runner `{}`: {error}", path.display()))
            .help("check the toolchain and executable permissions, then rerun the build")
            .finish(),
        ],
        BuildError::CompileRunner { error, .. } => match error.as_ref() {
            // This branch is only used by JSON callers. Text rendering above
            // delegates directly so source-owned driver labels are retained.
            nia_driver::DriverError::CheckDiagnostics(program) => program
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenProgramDiagnostics(program) => program
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenPreparationDiagnostics(diagnostics) => diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenDiagnostics(diagnostics) => diagnostics.clone(),
            _ => vec![
                Diagnostic::user_error(codes::BUILD_RUNNER, "could not compile the build runner")
                    .note(nia_driver::render_driver_error(error, None, None))
                    .help("fix the reported build.nia diagnostics before compiling again")
                    .finish(),
            ],
        },
        BuildError::CurrentDirectory { error } => io_diagnostic(
            codes::BUILD_ACTION,
            "could not determine the current directory",
            error,
            "run the build from a directory that still exists",
        ),
        BuildError::CreateBuildDirectory { path, error }
        | BuildError::CreateCacheDirectory { path, error }
        | BuildError::CreateRunnerDirectory { path, error } => vec![
            Diagnostic::user_error(codes::ARTIFACT_IO, "could not create a build directory")
                .note(format!("path `{}`: {error}", path.display()))
                .help("check directory permissions and available disk space")
                .finish(),
        ],
        BuildError::NonUtf8Path { role, path } => vec![
            Diagnostic::user_error(codes::BUILD_PLAN, "build path cannot be represented in Nia")
                .note(format!(
                    "{role} path `{}` is not valid UTF-8",
                    path.display()
                ))
                .help("rename the path to use UTF-8 characters")
                .finish(),
        ],
        BuildError::PreparePlanDraft { path, error } => plan_handoff_diagnostic(path, error),
        BuildError::PrepareRunnerConfiguration { path, error }
        | BuildError::CleanupRunnerConfiguration { path, error }
        | BuildError::CleanupPlanDraft { path, error }
        | BuildError::CleanupRunnerExecutable { path, error } => io_plan_diagnostic(path, error),
        BuildError::ReadPlanDraft { path, error }
        | BuildError::PublishBuildPlan { path, error } => plan_handoff_diagnostic(path, error),
        BuildError::RunnerConfigurationFieldTooLarge { role, len } => vec![
            Diagnostic::user_error(codes::BUILD_PLAN, "build-runner configuration is too large")
                .note(format!(
                    "{role} contains {len} bytes, exceeding the protocol limit"
                ))
                .help("shorten the configured path, argument, or environment value")
                .finish(),
        ],
        BuildError::RunnerConfigurationTooLarge { len } => vec![
            Diagnostic::user_error(codes::BUILD_PLAN, "build-runner configuration is too large")
                .note(format!("the encoded configuration contains {len} bytes"))
                .help("reduce the number or size of build configuration values")
                .finish(),
        ],
        BuildError::MissingBuildScript { start } => vec![
            Diagnostic::user_error(codes::BUILD_PLAN, "could not find `build.nia`")
                .note(format!(
                    "searched from `{}` and its package boundary",
                    start.display()
                ))
                .help("run the command inside a package containing build.nia, or pass its root")
                .finish(),
        ],
    }
}

fn coordinator_diagnostics(error: &CoordinatorError) -> Vec<Diagnostic> {
    if let CoordinatorError::Driver { error, .. } = error {
        return match error.as_ref() {
            nia_driver::DriverError::CheckDiagnostics(program) => program
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenProgramDiagnostics(program) => program
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenPreparationDiagnostics(diagnostics) => diagnostics
                .iter()
                .map(|diagnostic| diagnostic.diagnostic.clone())
                .collect(),
            nia_driver::DriverError::CodegenDiagnostics(diagnostics) => diagnostics.clone(),
            _ => vec![
                Diagnostic::user_error(codes::BUILD_ACTION, "compiler action failed")
                    .note(nia_driver::render_driver_error(error, None, None))
                    .finish(),
            ],
        };
    }
    vec![coordinator_diagnostic(error)]
}

fn coordinator_diagnostic(error: &CoordinatorError) -> Diagnostic {
    match error {
        CoordinatorError::Internal(ice) => Diagnostic::from(ice),
        CoordinatorError::Cancelled { action } => Diagnostic::user_error(
            codes::BUILD_ACTION,
            format!("build action `{}` was cancelled", action.name()),
        )
        .note("the action was stopped after another action failed")
        .finish(),
        CoordinatorError::TestFailures(failures) => Diagnostic::user_error(
            codes::BUILD_ACTION,
            format!("{} test suite(s) failed", failures.len()),
        )
        .note(test_failure_note(failures))
        .finish(),
        CoordinatorError::TargetMismatch(details) => Diagnostic::user_error(
            codes::BUILD_PLAN,
            "build plan target does not match the current invocation",
        )
        .note(format!(
            "expected {}, found {}",
            crate::coordinator::display_target(&details.expected),
            crate::coordinator::display_target(&details.found)
        ))
        .help("clean the build plan and rerun with the intended target")
        .finish(),
        CoordinatorError::InconsistentPlan { owner, missing } => {
            Diagnostic::user_error(codes::BUILD_PLAN, "frozen build plan is inconsistent")
                .note(format!("{owner} references missing {missing}"))
                .help("remove the stale build plan and regenerate it")
                .finish()
        }
        CoordinatorError::UnmappedPackage { action, package } => Diagnostic::user_error(
            codes::BUILD_PLAN,
            "build action references an unmapped package",
        )
        .note(format!(
            "action `{}` uses package `{}`",
            action.name(),
            package.as_str()
        ))
        .finish(),
        CoordinatorError::InvalidModuleImport(details) => Diagnostic::user_error(
            codes::BUILD_PLAN,
            format!("cannot map build import `{}`", details.name),
        )
        .note(details.reason.clone())
        .finish(),
        CoordinatorError::NonUtf8Path { action, path } => Diagnostic::user_error(
            codes::BUILD_ACTION,
            "build action resolved a non-UTF-8 path",
        )
        .note(format!(
            "action `{}` resolved `{}`",
            action.name(),
            path.display()
        ))
        .help("rename the path to use UTF-8 characters")
        .finish(),
        CoordinatorError::GeneratedFileIo {
            action,
            path,
            operation,
            error,
        }
        | CoordinatorError::InstallArtifactIo {
            action,
            path,
            operation,
            error,
        }
        | CoordinatorError::StaticArchiveLinkInputIo {
            action,
            path,
            operation,
            error,
        }
        | CoordinatorError::ExternalCommandIo {
            action,
            path,
            operation,
            error,
        } => Diagnostic::user_error(
            codes::BUILD_ACTION,
            format!("build action `{}` failed to {operation}", action.name()),
        )
        .note(format!("path `{}`: {error}", path.display()))
        .help("check the input/output path and available permissions")
        .finish(),
        CoordinatorError::ExternalCommand(details) => external_command_diagnostic(details),
        CoordinatorError::StagedOutput {
            action,
            path,
            operation,
            error,
            cause,
        } => {
            let mut diagnostic = Diagnostic::user_error(
                codes::BUILD_ACTION,
                format!(
                    "build action `{}` failed to {operation} staged output",
                    action.name()
                ),
            )
            .note(format!("path `{}`: {error}", path.display()));
            if let Some(cause) = cause {
                diagnostic = diagnostic.note(format!(
                    "original action failure: {}",
                    coordinator_summary(cause)
                ));
            }
            diagnostic.finish()
        }
        CoordinatorError::AcquireOutputLock {
            action,
            output,
            lock,
            error,
        } => Diagnostic::user_error(
            codes::BUILD_ACTION,
            format!(
                "build action `{}` could not publish its output",
                action.name()
            ),
        )
        .note(format!(
            "output `{}` via lock `{}`: {error}",
            output.display(),
            lock.display()
        ))
        .finish(),
        CoordinatorError::OutputRecovery(error) => Diagnostic::user_error(
            codes::BUILD_ACTION,
            "could not recover an interrupted build output",
        )
        .note(error.to_string())
        .finish(),
        CoordinatorError::UnsupportedAction { action, kind } => Diagnostic::user_error(
            codes::BUILD_ACTION,
            format!("build action `{}` is unsupported", action.name()),
        )
        .note(format!("action kind: {kind}"))
        .finish(),
        CoordinatorError::Driver { .. } => unreachable!("driver errors are handled above"),
    }
}

fn external_command_diagnostic(details: &ExternalCommandError) -> Diagnostic {
    let action = details.action.name();
    let mut diagnostic = Diagnostic::user_error(
        codes::BUILD_ACTION,
        format!("external command action `{action}` failed"),
    )
    .note(format!(
        "program `{}` in `{}`",
        details.program,
        details.working_directory.display()
    ))
    .finish();
    match &details.failure {
        ExternalCommandFailure::Spawn { error }
        | ExternalCommandFailure::Wait { error }
        | ExternalCommandFailure::CaptureWorkerSpawn { error, .. }
        | ExternalCommandFailure::StreamIo { error, .. } => {
            diagnostic.notes.push(error.to_string());
        }
        ExternalCommandFailure::MissingPipe { stream } => {
            diagnostic
                .notes
                .push(format!("the configured {stream} pipe was unavailable"));
        }
        ExternalCommandFailure::CaptureThread { stream } => {
            diagnostic
                .notes
                .push(format!("the {stream} capture worker failed"));
        }
        ExternalCommandFailure::TimedOut {
            timeout,
            stdout,
            stderr,
        } => {
            let reason = format!(
                "command timed out after {}",
                crate::coordinator::display_duration(*timeout)
            );
            diagnostic.notes.push(reason);
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
        ExternalCommandFailure::Cancelled { stdout, stderr } => {
            diagnostic
                .notes
                .push("command was cancelled after another action failed".to_string());
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
        ExternalCommandFailure::Exit {
            status,
            stdout,
            stderr,
        } => {
            let reason = format!("command exited with status {status}");
            diagnostic.notes.push(reason);
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
    }
    diagnostic
        .help
        .push("inspect the command output and fix the external build step".to_string());
    diagnostic
}

fn runner_failure(
    path: &Path,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Diagnostic {
    let mut diagnostic =
        Diagnostic::user_error(codes::BUILD_RUNNER, "build runner exited unsuccessfully")
            .note(format!(
                "runner `{}` exited with status {status}",
                path.display()
            ))
            .finish();
    append_output_note(&mut diagnostic, "stdout", stdout);
    append_output_note(&mut diagnostic, "stderr", stderr);
    diagnostic
        .help
        .push("fix the build.nia action that failed, then rerun the build".to_string());
    diagnostic
}

fn append_output_note(diagnostic: &mut Diagnostic, stream: &str, bytes: &[u8]) {
    if !bytes.is_empty() {
        diagnostic.notes.push(format!(
            "{stream} tail (bounded):\n{}",
            String::from_utf8_lossy(bytes)
        ));
    }
}

fn io_diagnostic(
    code: nia_diagnostic::codes::DiagnosticCodeDef,
    summary: &str,
    error: &std::io::Error,
    help: &str,
) -> Vec<Diagnostic> {
    vec![
        Diagnostic::user_error(code, summary)
            .note(error.to_string())
            .help(help)
            .finish(),
    ]
}

fn io_plan_diagnostic(path: &Path, error: &std::io::Error) -> Vec<Diagnostic> {
    vec![
        Diagnostic::user_error(codes::BUILD_PLAN, "could not complete build handoff")
            .note(format!("path `{}`: {error}", path.display()))
            .help("remove stale .nia-build files if necessary and check permissions")
            .finish(),
    ]
}

fn plan_handoff_diagnostic<T: std::fmt::Display>(path: &Path, error: &T) -> Vec<Diagnostic> {
    vec![
        Diagnostic::user_error(codes::BUILD_PLAN, "could not complete build handoff")
            .note(format!("path `{}`: {error}", path.display()))
            .help("remove stale .nia-build files if necessary and check permissions")
            .finish(),
    ]
}

fn test_failure_note(failures: &[TestFailure]) -> String {
    failures
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure.action.name(),
                coordinator_summary(&failure.error)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn coordinator_summary(error: &CoordinatorError) -> String {
    match error {
        CoordinatorError::ExternalCommand(details) => {
            format!("external command `{}` failed", details.program)
        }
        CoordinatorError::Driver { .. } => "compiler action failed".to_string(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetSpec;

    #[test]
    fn missing_script_uses_stable_build_plan_code_and_help() {
        let error = BuildError::MissingBuildScript {
            start: std::path::PathBuf::from("/tmp/project/src"),
        };
        let rendered = render_build_error(&error, None, None);
        assert!(rendered.contains("error[E0704]: could not find `build.nia`"));
        assert!(rendered.contains("help: run the command inside a package"));
    }

    #[test]
    fn build_plan_json_is_machine_readable_and_stable() {
        let error = BuildError::MissingBuildScript {
            start: std::path::PathBuf::from("/tmp/project/src"),
        };
        let json = render_build_error_json(&error);
        assert!(json.starts_with("{\"diagnostics\":["), "{json}");
        assert!(json.contains("\"code\":\"E0704\""), "{json}");
        assert!(json.contains("\"help\":["), "{json}");
        assert!(json.contains("\"suppressed\":{"), "{json}");
    }

    #[test]
    fn target_mismatch_uses_semantic_target_text() {
        let expected = TargetSpec {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: "gnu".to_string(),
            abi: "".to_string(),
            endian: "little".to_string(),
            pointer_width: 64,
        };
        let mut found = expected.clone();
        found.arch = "aarch64".to_string();
        let error = BuildError::ExecuteBuildPlan {
            error: Box::new(CoordinatorError::TargetMismatch(Box::new(
                crate::coordinator::TargetMismatch {
                    role: "host",
                    expected,
                    found,
                },
            ))),
        };

        let rendered = render_build_error(&error, None, None);
        assert!(rendered.contains(
            "note: expected x86_64-unknown-linux-gnu- (64-bit little), found aarch64-unknown-linux-gnu- (64-bit little)"
        ));
        assert!(!rendered.contains("TargetSpec {"));
        assert!(!rendered.contains("pointer_width:"));
    }

    #[test]
    fn target_mismatch_json_uses_semantic_target_text() {
        let expected = TargetSpec {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: "gnu".to_string(),
            abi: "".to_string(),
            endian: "little".to_string(),
            pointer_width: 64,
        };
        let mut found = expected.clone();
        found.arch = "aarch64".to_string();
        let error = BuildError::ExecuteBuildPlan {
            error: Box::new(CoordinatorError::TargetMismatch(Box::new(
                crate::coordinator::TargetMismatch {
                    role: "host",
                    expected,
                    found,
                },
            ))),
        };

        let json = render_build_error_json(&error);
        assert!(json.contains("expected x86_64-unknown-linux-gnu- (64-bit little), found aarch64-unknown-linux-gnu- (64-bit little)"));
        assert!(!json.contains("TargetSpec {"));
        assert!(!json.contains("pointer_width:"));
    }

    #[test]
    fn external_command_timeout_uses_semantic_duration_everywhere() {
        let details = ExternalCommandError {
            action: crate::ActionKey::new(crate::PackageKey::root(), "compile").unwrap(),
            program: "cc".to_string(),
            arguments: Vec::new(),
            working_directory: std::path::PathBuf::from("/tmp/project"),
            failure: ExternalCommandFailure::TimedOut {
                timeout: std::time::Duration::from_secs(420),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        };

        let diagnostic = external_command_diagnostic(&details);
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note == "command timed out after 7m")
        );
        let rendered = render_diagnostic_list("build diagnostics:", &[diagnostic], None, None);
        assert!(rendered.contains("command timed out after 7m"));

        let coordinator = CoordinatorError::ExternalCommand(Box::new(details));
        let displayed = coordinator.to_string();
        assert!(displayed.contains("run `cc`"));
        assert!(displayed.contains("timed out after 7m"));
        assert!(!displayed.contains("420s"));
    }

    #[cfg(unix)]
    #[test]
    fn runner_failure_keeps_bounded_stream_context() {
        use std::process::Command;

        let status = Command::new("sh")
            .args(["-c", "printf runner-out; printf runner-err >&2; exit 7"])
            .status()
            .expect("run test runner");
        let error = BuildError::RunnerFailed {
            path: std::path::PathBuf::from(".nia-build/runner"),
            status,
            stdout: b"runner-out".to_vec(),
            stderr: b"runner-err".to_vec(),
        };
        let rendered = render_build_error(&error, None, None);
        assert!(rendered.contains("error[E0703]: build runner exited unsuccessfully"));
        assert!(rendered.contains("stdout tail (bounded):"));
        assert!(rendered.contains("stderr tail (bounded):"));
        assert!(rendered.contains("help: fix the build.nia action"));
    }

    #[cfg(unix)]
    #[test]
    fn runner_failure_json_keeps_bounded_output_notes() {
        use std::process::Command;

        let status = Command::new("sh")
            .args(["-c", "exit 7"])
            .status()
            .expect("run test runner");
        let error = BuildError::RunnerFailed {
            path: std::path::PathBuf::from(".nia-build/runner"),
            status,
            stdout: b"stdout-tail".to_vec(),
            stderr: b"stderr-tail".to_vec(),
        };
        let json = render_build_error_json(&error);
        assert!(json.contains("\"code\":\"E0703\""), "{json}");
        assert!(json.contains("stdout-tail"), "{json}");
        assert!(json.contains("stderr-tail"), "{json}");
        assert!(json.contains("\"suppressed\":{"), "{json}");
    }
}
