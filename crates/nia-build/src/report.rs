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
    if let BuildError::CompileRunner {
        path,
        source,
        error,
    } = error
    {
        // Driver diagnostics already contain the source-owned build.nia
        // locations. Do not wrap them in a synthetic runner diagnostic.
        return nia_driver::render_driver_error(error, Some(path), Some(source));
    }
    if let BuildError::ExecuteBuildPlan { error } = error
        && let CoordinatorError::Driver { error, .. } = error.as_ref()
    {
        // Action diagnostics carry their own source paths. Preserve that
        // ownership instead of rendering them as a generic build error.
        return nia_driver::render_driver_error(error, primary_path, primary_source);
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
    if let BuildError::ExecuteBuildPlan { error } = error
        && let CoordinatorError::Driver { error, .. } = error.as_ref()
    {
        return nia_driver::render_driver_error_json(error);
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
    let error_label = if report.error_count() == 1 {
        "error"
    } else {
        "errors"
    };
    let warning_label = if report.warning_count() == 1 {
        "warning"
    } else {
        "warnings"
    };
    let _ = writeln!(
        output,
        "summary: {} {error_label}, {} {warning_label}",
        report.error_count(),
        report.warning_count()
    );
    if report.suppressed_duplicates() > 0 || report.suppressed_by_limit() > 0 {
        let _ = writeln!(
            output,
            "note: suppressed {} diagnostic(s) ({} duplicate(s), {} over limit)",
            report.suppressed_duplicates() + report.suppressed_by_limit(),
            report.suppressed_duplicates(),
            report.suppressed_by_limit()
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
        BuildError::RunRunner {
            path,
            error,
            stdout,
            stderr,
        } => {
            let mut diagnostic = Diagnostic::user_error(
                codes::BUILD_RUNNER,
                "could not run the generated build runner",
            )
            .note(format!("runner `{}`: {error}", path.display()))
            .help("check the toolchain and executable permissions, then rerun the build")
            .finish();
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
            vec![diagnostic]
        }
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
            nia_driver::DriverError::CodegenPreparationDiagnostics { diagnostics, .. } => {
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.diagnostic.clone())
                    .collect()
            }
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
        BuildError::CreateBuildDirectory { path, error } => {
            create_directory_diagnostic("build directory", path, error)
        }
        BuildError::CreateCacheDirectory { path, error } => {
            create_directory_diagnostic("build cache directory", path, error)
        }
        BuildError::CreateRunnerDirectory { path, error } => {
            create_directory_diagnostic("build runner directory", path, error)
        }
        BuildError::NonUtf8Path { role, path } => vec![
            Diagnostic::user_error(codes::BUILD_PLAN, "build path cannot be represented in Nia")
                .note(format!(
                    "{role} path `{}` is not valid UTF-8",
                    path.display()
                ))
                .help("rename the path to use UTF-8 characters")
                .finish(),
        ],
        BuildError::PreparePlanDraft { path, error } => {
            io_plan_diagnostic("remove previous plan draft", path, error)
        }
        BuildError::PrepareRunnerConfiguration { path, error } => {
            io_plan_diagnostic("write runner configuration", path, error)
        }
        BuildError::CleanupRunnerConfiguration { path, error } => {
            io_plan_diagnostic("remove runner configuration", path, error)
        }
        BuildError::CleanupPlanDraft { path, error } => {
            io_plan_diagnostic("remove plan draft", path, error)
        }
        BuildError::CleanupRunnerExecutable { path, error } => {
            io_plan_diagnostic("remove runner executable", path, error)
        }
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
            nia_driver::DriverError::CodegenPreparationDiagnostics { diagnostics, .. } => {
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.diagnostic.clone())
                    .collect()
            }
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
        CoordinatorError::TestFailures(failures) => {
            let mut diagnostic = Diagnostic::user_error(
                codes::BUILD_ACTION,
                format!("{} test suite(s) failed", failures.len()),
            )
            .note(test_failure_note(failures))
            .finish();
            for failure in failures {
                if let CoordinatorError::ExternalCommand(details) = failure.error.as_ref() {
                    let child = external_command_diagnostic(details);
                    for note in child.notes.iter() {
                        diagnostic
                            .notes
                            .push(format!("test suite `{}`: {note}", failure.action.name()));
                    }
                }
            }
            diagnostic
        }
        CoordinatorError::TargetMismatch(details) => Diagnostic::user_error(
            codes::BUILD_PLAN,
            format!(
                "build plan {} target does not match the current invocation",
                details.role
            ),
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
                diagnostic = diagnostic.note(format!("original action failure: {}", cause));
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
        ExternalCommandFailure::Spawn { error } => {
            diagnostic
                .notes
                .push(format!("command spawn failed: {error}"));
        }
        ExternalCommandFailure::Wait {
            error,
            stdout,
            stderr,
        } => {
            diagnostic
                .notes
                .push(format!("command wait failed: {error}"));
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
        ExternalCommandFailure::CaptureWorkerSpawn {
            stream,
            error,
            stdout,
            stderr,
        } => {
            diagnostic
                .notes
                .push(format!("failed to start {stream} capture worker: {error}"));
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
        ExternalCommandFailure::StreamIo {
            stream,
            error,
            stdout,
            stderr,
        } => {
            diagnostic
                .notes
                .push(format!("{stream} capture/forward failed: {error}"));
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
        }
        ExternalCommandFailure::MissingPipe { stream } => {
            diagnostic
                .notes
                .push(format!("the configured {stream} pipe was unavailable"));
        }
        ExternalCommandFailure::CaptureThread {
            stream,
            stdout,
            stderr,
        } => {
            diagnostic
                .notes
                .push(format!("the {stream} capture worker failed"));
            append_output_note(&mut diagnostic, "stdout", stdout);
            append_output_note(&mut diagnostic, "stderr", stderr);
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

fn create_directory_diagnostic(role: &str, path: &Path, error: &std::io::Error) -> Vec<Diagnostic> {
    vec![
        Diagnostic::user_error(codes::ARTIFACT_IO, format!("could not create {role}"))
            .note(format!("path `{}`: {error}", path.display()))
            .help("check directory permissions and available disk space")
            .finish(),
    ]
}

fn io_plan_diagnostic(operation: &str, path: &Path, error: &std::io::Error) -> Vec<Diagnostic> {
    vec![
        Diagnostic::user_error(codes::BUILD_PLAN, "could not complete build handoff")
            .note(format!(
                "failed to {operation} at `{}`: {error}",
                path.display()
            ))
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
pub(crate) mod tests {
    use super::*;
    use crate::TargetSpec;

    pub(crate) fn assert_operational_report(error: &BuildError, code: &str, markers: &[&str]) {
        let text = render_build_error(error, None, None);
        let json = render_build_error_json(error);
        let report: serde_json::Value = serde_json::from_str(&json).expect("complete report JSON");
        assert!(text.contains(&format!("error[{code}]")), "{text}");
        assert!(!text.contains('\x1b'), "{text}");
        let diagnostics = report["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{json}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic["code"], code, "{json}");
        assert_eq!(diagnostic["severity"], "error", "{json}");
        assert_eq!(diagnostic["category"], "user", "{json}");
        assert_eq!(diagnostic["kind"], "independent", "{json}");
        assert!(
            diagnostic["labels"].as_array().unwrap().is_empty(),
            "{json}"
        );
        assert_eq!(report["summary"]["errors"], 1, "{json}");
        assert_eq!(report["summary"]["warnings"], 0, "{json}");
        let notes = diagnostic["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|note| note.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let summary = diagnostic["summary"].as_str().unwrap();
        for marker in markers {
            assert!(text.contains(marker), "missing {marker:?}: {text}");
            assert!(
                summary.contains(marker) || notes.contains(marker),
                "missing {marker:?}: {json}"
            );
        }
    }

    #[test]
    fn runner_related_locations_use_generated_source_instead_of_caller_text() {
        let runner_path = "build-package:build-runner:/root.nia";
        let source_path = "missing-build-source.nia";
        let diagnostic = nia_driver::ProgramDiagnostic {
            path: nia_driver::SourcePath::new(source_path),
            diagnostic: Diagnostic::user_error(codes::NAME_RESOLUTION, "source-owned error")
                .primary(nia_span::Span::new(2, 5), "source failure")
                .related_at(runner_path, nia_span::Span::new(9, 13), "generated call")
                .finish(),
        };
        let error = BuildError::CompileRunner {
            path: runner_path.to_owned(),
            source: "// auto\n\ncall();\n".to_owned(),
            error: Box::new(nia_driver::DriverError::CodegenPreparationDiagnostics {
                diagnostic_sources: Default::default(),
                diagnostics: vec![diagnostic],
                suppressed_downstream: 0,
            }),
        };
        let text = render_build_error(&error, Some(source_path), Some("unrelated caller text"));
        assert!(text.contains("error[E0201]: source-owned error"), "{text}");
        assert!(
            text.contains(&format!("related: {runner_path}:3:1: generated call")),
            "{text}"
        );
        assert!(!text.contains("unrelated caller text"), "{text}");
        let json = render_build_error_json(&error);
        assert!(
            json.contains(source_path) && json.contains(runner_path),
            "{json}"
        );
    }

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
    fn directory_failures_keep_their_resource_role() {
        let cache = BuildError::CreateCacheDirectory {
            path: std::path::PathBuf::from("/tmp/project/.nia-cache"),
            error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let runner = BuildError::CreateRunnerDirectory {
            path: std::path::PathBuf::from("/tmp/project/.nia-runner"),
            error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };

        let cache_rendered = render_build_error(&cache, None, None);
        assert!(
            cache_rendered.contains("could not create build cache directory"),
            "{cache_rendered}"
        );
        assert!(!cache_rendered.contains("could not create a build directory"));

        let runner_rendered = render_build_error_json(&runner);
        assert!(
            runner_rendered.contains("could not create build runner directory"),
            "{runner_rendered}"
        );
        assert!(!runner_rendered.contains("could not create a build directory"));
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

    #[test]
    fn unavailable_capture_workers_report_stage_stream_and_available_output() {
        for stream in ["stdout", "stderr"] {
            for missing_pipe in [false, true] {
                let failure = if missing_pipe {
                    ExternalCommandFailure::MissingPipe { stream }
                } else {
                    ExternalCommandFailure::CaptureWorkerSpawn {
                        stream,
                        error: std::io::Error::other("injected thread-spawn failure"),
                        stdout: if stream == "stderr" {
                            b"stdout-tail".to_vec()
                        } else {
                            Vec::new()
                        },
                        stderr: Vec::new(),
                    }
                };
                let reason = if missing_pipe {
                    format!("configured {stream} pipe was unavailable")
                } else {
                    format!(
                        "failed to start {stream} capture worker: injected thread-spawn failure"
                    )
                };
                let mut markers = vec![reason.as_str()];
                if !missing_pipe && stream == "stderr" {
                    markers.push("stdout-tail");
                }
                let error = BuildError::ExecuteBuildPlan {
                    error: Box::new(CoordinatorError::ExternalCommand(Box::new(
                        ExternalCommandError {
                            action: crate::ActionKey::new(crate::PackageKey::root(), "capture")
                                .unwrap(),
                            program: "tool".into(),
                            arguments: Vec::new(),
                            working_directory: "work".into(),
                            failure,
                        },
                    ))),
                };
                assert_operational_report(&error, "E0705", &markers);
            }
        }
    }

    #[test]
    fn suite_failures_keep_each_captured_stream_and_failure_reason() {
        let failures = ["first", "second"]
            .into_iter()
            .map(|name| {
                let action = crate::ActionKey::new(crate::PackageKey::root(), name).unwrap();
                TestFailure {
                    action: action.clone(),
                    error: Box::new(CoordinatorError::ExternalCommand(Box::new(
                        ExternalCommandError {
                            action,
                            program: name.to_owned(),
                            arguments: Vec::new(),
                            working_directory: std::path::PathBuf::from("tests"),
                            failure: ExternalCommandFailure::TimedOut {
                                timeout: std::time::Duration::from_secs(2),
                                stdout: format!("{name}-stdout").into_bytes(),
                                stderr: format!("{name}-stderr").into_bytes(),
                            },
                        },
                    ))),
                }
            })
            .collect();
        let diagnostic = coordinator_diagnostic(&CoordinatorError::TestFailures(failures));
        assert_eq!(diagnostic.summary, "2 test suite(s) failed");
        assert!(diagnostic.labels.is_empty());
        for name in ["first", "second"] {
            for note in [
                format!("test suite `{name}`: command timed out after 2s"),
                format!("test suite `{name}`: stdout tail (bounded):\n{name}-stdout"),
                format!("test suite `{name}`: stderr tail (bounded):\n{name}-stderr"),
            ] {
                assert!(diagnostic.notes.contains(&note), "{:?}", diagnostic.notes);
            }
        }
        let text = render_diagnostic_list(
            "build diagnostics:",
            std::slice::from_ref(&diagnostic),
            None,
            None,
        );
        let json = nia_diagnostic::render_diagnostic_json("<unknown>", &diagnostic);
        for marker in [
            "first-stdout",
            "first-stderr",
            "second-stdout",
            "second-stderr",
        ] {
            assert!(text.contains(marker), "{text}");
            assert!(json.contains(marker), "{json}");
        }
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
