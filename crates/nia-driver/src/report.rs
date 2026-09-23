// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{
    BackendOptimizationChange, CheckedProgram, CodegenProgram, DriverError, LlvmIrArtifact,
    NiaOptimizationLevel, ObjectArtifact,
};
use nia_diagnostic::{
    Diagnostic, DiagnosticReportConfig, DiagnosticReportEntryKind, DiagnosticReportItem,
    build_diagnostic_report, render_diagnostic, render_diagnostics_json,
    render_diagnostics_json_at,
};
use nia_opt::{InlineThreshold, OptimizationDepth, SpecializationPolicy};

/// Renders the backend optimization report with a trailing newline.
pub fn optimization_report(program: &CodegenProgram) -> String {
    let mut out = optimization_report_lines_from_parts(
        program.optimization,
        &program.backend_lowering.optimization_report,
    )
    .join("\n");
    out.push('\n');
    out
}

/// Returns optimization report lines without terminal newline formatting.
pub fn optimization_report_lines(program: &CodegenProgram) -> Vec<String> {
    optimization_report_lines_from_parts(
        program.optimization,
        &program.backend_lowering.optimization_report,
    )
}

/// Renders optimization changes recorded for LLVM IR emission.
pub fn llvm_ir_optimization_report(artifact: &LlvmIrArtifact) -> String {
    let mut out =
        optimization_report_lines_from_parts(artifact.optimization, &artifact.optimization_report)
            .join("\n");
    out.push('\n');
    out
}

/// Renders optimization changes recorded for native object emission.
pub fn object_optimization_report(artifact: &ObjectArtifact) -> String {
    let mut out =
        optimization_report_lines_from_parts(artifact.optimization, &artifact.optimization_report)
            .join("\n");
    out.push('\n');
    out
}

/// Renders a report from an explicit policy and backend change product.
pub fn optimization_report_from_parts(
    optimization: crate::OptimizationPolicy,
    report: &crate::BackendOptimizationReport,
) -> String {
    let mut out = optimization_report_lines_from_parts(optimization, report).join("\n");
    out.push('\n');
    out
}

fn optimization_report_lines_from_parts(
    policy: crate::OptimizationPolicy,
    report: &crate::BackendOptimizationReport,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("backend optimization report:".to_string());
    lines.push(format!(
        "  policy level={} simplify_cfg={} const_fold={} dead_code_elim={} \
         local_copy_prop={} inline={} specialize={} dedup_monomorphized_instances={} \
         prefer_size={} llvm_codegen={} llvm_size={}",
        optimization_level_name(policy.level),
        optimization_depth_name(policy.simplify_cfg),
        optimization_depth_name(policy.const_fold),
        optimization_depth_name(policy.dead_code_elim),
        optimization_depth_name(policy.local_copy_prop),
        inline_threshold_name(policy.inline_threshold),
        specialization_policy_name(policy.specialize_generics),
        policy.dedup_monomorphized_instances,
        policy.prefer_size,
        nia_codegen_llvm::llvm_codegen_optimization_level(policy.level).name(),
        nia_codegen_llvm::llvm_codegen_size_policy(policy.level).name()
    ));
    lines.push(format!(
        "  enabled_module_passes={}",
        enabled_passes_name(&report.enabled_module_passes)
    ));
    lines.push(format!(
        "  enabled_function_passes={}",
        enabled_passes_name(&report.enabled_function_passes)
    ));
    lines.push(format!(
        "  enabled_global_passes={}",
        enabled_passes_name(&report.enabled_global_passes)
    ));
    lines.push(format!("  changes={}", report.changed_passes.len()));
    if report.changed_passes.is_empty() {
        lines.push("  no changes".to_string());
        return lines;
    }
    for change in &report.changed_passes {
        match change {
            BackendOptimizationChange::Function {
                function,
                pass,
                is_instance,
                type_arg_count,
                ..
            } => {
                let instance = if *is_instance { " instance" } else { "" };
                lines.push(format!(
                    "  m{}::d{}{} {} type_args={}",
                    function.module_id.local_index(),
                    function.def_id.0,
                    instance,
                    pass,
                    type_arg_count
                ));
            }
            BackendOptimizationChange::Global { global, pass, .. } => {
                lines.push(format!(
                    "  m{}::d{} global {}",
                    global.module_id.local_index(),
                    global.def_id.0,
                    pass
                ));
            }
        }
    }
    lines
}

/// Renders all diagnostics attached to a checked program.
pub fn render_program_diagnostics(
    program: &CheckedProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    render_program_diagnostic_items(
        &program.diagnostics,
        program.suppressed_downstream,
        primary_path,
        primary_source,
    )
}

/// Renders all diagnostics attached to a checked program as deterministic JSON.
pub fn render_program_diagnostics_json(program: &CheckedProgram) -> String {
    let diagnostics = program
        .diagnostics
        .iter()
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    nia_diagnostic::render_diagnostics_json_with_downstream(
        &diagnostics,
        DiagnosticReportConfig::default(),
        program.suppressed_downstream,
    )
}

/// Renders only warnings attached to a checked program.
pub fn render_program_warnings(
    program: &CheckedProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .cloned()
        .collect::<Vec<_>>();
    render_program_diagnostic_items(&diagnostics, 0, primary_path, primary_source)
}

/// Renders only checked-program warnings as deterministic JSON.
pub fn render_program_warnings_json(program: &CheckedProgram) -> String {
    let diagnostics = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    render_diagnostics_json(&diagnostics, DiagnosticReportConfig::default())
}

/// Renders only warnings attached to an LLVM artifact.
pub fn render_llvm_ir_warnings(
    artifact: &LlvmIrArtifact,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = artifact
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .cloned()
        .collect::<Vec<_>>();
    render_program_diagnostic_items(&diagnostics, 0, primary_path, primary_source)
}

/// Renders only warnings attached to an object artifact.
pub fn render_object_warnings(
    artifact: &ObjectArtifact,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = artifact
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .cloned()
        .collect::<Vec<_>>();
    render_program_diagnostic_items(&diagnostics, 0, primary_path, primary_source)
}

/// Renders only warnings attached to a linked executable artifact.
pub fn render_executable_warnings(
    artifact: &crate::ExecutableArtifact,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = artifact
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .cloned()
        .collect::<Vec<_>>();
    render_program_diagnostic_items(&diagnostics, 0, primary_path, primary_source)
}

fn render_program_diagnostic_items(
    diagnostics: &[crate::ProgramDiagnostic],
    suppressed_downstream: usize,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let sources = diagnostics
        .iter()
        .map(|diagnostic| {
            let path = diagnostic.path.as_str().to_owned();
            let source = nia_source::read_source_text(&path).unwrap_or_default();
            (path, source)
        })
        .collect::<HashMap<_, _>>();
    let diagnostics = diagnostics
        .iter()
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    let report = nia_diagnostic::build_diagnostic_report_with_downstream(
        &diagnostics,
        DiagnosticReportConfig::default(),
        suppressed_downstream,
    );
    let mut out = String::new();
    out.push_str("diagnostics:\n");
    for (index, entry) in report.entries().iter().enumerate() {
        match report.entry_kind(index) {
            Some(DiagnosticReportEntryKind::Root) => out.push_str("root diagnostic:\n"),
            Some(DiagnosticReportEntryKind::Related) => {
                let parent = report
                    .entry_parent(index)
                    .expect("related report entry must have a root");
                out.push_str(&format!("related to root diagnostic {}:\n", parent + 1));
            }
            Some(DiagnosticReportEntryKind::Independent) => {
                out.push_str("independent diagnostic:\n")
            }
            None => unreachable!("report entry kind must match retained entry"),
        }
        let source = diagnostic_source(entry.path, primary_path, primary_source);
        out.push_str(&nia_diagnostic::render_diagnostic_with_sources(
            entry.path,
            &source,
            entry.diagnostic,
            &sources,
        ));
        out.push('\n');
    }
    push_report_summary(&mut out, &report);
    out
}

/// Renders diagnostics attached to a codegen program.
pub fn render_codegen_program_diagnostics(
    program: &CodegenProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    render_program_diagnostic_items(
        &program.diagnostics,
        program.suppressed_downstream,
        primary_path,
        primary_source,
    )
}

/// Renders warnings attached to a codegen program.
pub fn render_codegen_program_warnings(
    program: &CodegenProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .cloned()
        .collect::<Vec<_>>();
    render_program_diagnostic_items(&diagnostics, 0, primary_path, primary_source)
}

/// Renders only codegen-program warnings as deterministic JSON.
pub fn render_codegen_program_warnings_json(program: &CodegenProgram) -> String {
    let diagnostics = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    render_diagnostics_json(&diagnostics, DiagnosticReportConfig::default())
}

/// Renders a driver failure, preserving structured diagnostic details.
pub fn render_driver_error(
    error: &DriverError,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    match error {
        DriverError::ArchiveStatus {
            program,
            status,
            stderr,
        } => render_external_tool_diagnostic(
            "archive tool",
            program,
            *status,
            stderr,
            primary_path,
            primary_source,
        ),
        DriverError::ArchiveIo { program, error } => render_external_tool_io_diagnostic(
            "archive tool",
            program,
            error,
            primary_path,
            primary_source,
        ),
        DriverError::ArchiveConfig(error) => render_linker_config_diagnostic(
            "archive tool",
            &error.to_string(),
            primary_path,
            primary_source,
        ),
        DriverError::CheckDiagnostics(program) => {
            render_program_diagnostics(program, primary_path, primary_source)
        }
        DriverError::CodegenProgramDiagnostics(program) => {
            render_codegen_program_diagnostics(program, primary_path, primary_source)
        }
        DriverError::CodegenPreparationDiagnostics {
            diagnostics,
            suppressed_downstream,
        } => render_program_diagnostic_items(
            diagnostics,
            *suppressed_downstream,
            primary_path,
            primary_source,
        ),
        DriverError::CodegenDiagnostics(diagnostics) => {
            render_codegen_diagnostics(diagnostics, primary_path, primary_source)
        }
        DriverError::InternalDiagnostic(diagnostic) => render_diagnostics_with_title(
            "internal diagnostics:",
            std::slice::from_ref(diagnostic),
            primary_path,
            primary_source,
        ),
        DriverError::InvalidArtifactRequest(message) => {
            let diagnostic = Diagnostic::user_error(nia_diagnostic::codes::TARGET_CONFIG, message)
                .help("choose a supported artifact mode and output combination")
                .finish();
            render_diagnostics_with_title(
                "driver diagnostics:",
                std::slice::from_ref(&diagnostic),
                primary_path,
                primary_source,
            )
        }
        DriverError::Runtime(error) => {
            let diagnostic = Diagnostic::user_error(
                nia_diagnostic::codes::TARGET_CONFIG,
                "invalid runtime configuration",
            )
            .note(error.to_string())
            .help("select a runtime supported by the artifact target")
            .finish();
            render_diagnostics_with_title(
                "driver diagnostics:",
                std::slice::from_ref(&diagnostic),
                primary_path,
                primary_source,
            )
        }
        DriverError::Io {
            path,
            operation,
            error,
        } => render_artifact_io_diagnostic(path, operation, error, primary_path, primary_source),
        DriverError::LinkerStatus {
            program,
            status,
            stderr,
        } => render_external_tool_diagnostic(
            "linker",
            program,
            *status,
            stderr,
            primary_path,
            primary_source,
        ),
        DriverError::LinkerIo { program, error } => render_external_tool_io_diagnostic(
            "linker",
            program,
            error,
            primary_path,
            primary_source,
        ),
        DriverError::LinkerConfig(error) => render_linker_config_diagnostic(
            "linker",
            &error.to_string(),
            primary_path,
            primary_source,
        ),
    }
}

fn render_external_tool_diagnostic(
    tool_kind: &str,
    program: &str,
    status: std::process::ExitStatus,
    stderr: &str,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let mut diagnostic = Diagnostic::user_error(
        nia_diagnostic::codes::LINKER,
        format!("{tool_kind} `{program}` failed"),
    )
    .note(format!("the {tool_kind} exited with status {status}"))
    .help(format!(
        "inspect the {tool_kind} inputs and the tool output below, then fix the missing or conflicting native dependency"
    ))
    .debug("program", program)
    .debug("exit_status", status);
    if !stderr.is_empty() {
        diagnostic = diagnostic.note(format!("{tool_kind} output:\n{stderr}"));
    }
    render_diagnostics_with_title(
        &format!("{tool_kind} diagnostics:"),
        std::slice::from_ref(&diagnostic.finish()),
        primary_path,
        primary_source,
    )
}

fn render_external_tool_io_diagnostic(
    tool_kind: &str,
    program: &str,
    error: &std::io::Error,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostic = Diagnostic::user_error(
        nia_diagnostic::codes::LINKER,
        format!("could not start {tool_kind} `{program}`"),
    )
    .note(format!("the operating system reported: {error}"))
    .help(format!(
        "install the {tool_kind}, make it executable, or select a different tool"
    ))
    .finish();
    render_diagnostics_with_title(
        &format!("{tool_kind} diagnostics:"),
        std::slice::from_ref(&diagnostic),
        primary_path,
        primary_source,
    )
}

fn render_linker_config_diagnostic(
    tool_kind: &str,
    error: &str,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostic = Diagnostic::user_error(
        nia_diagnostic::codes::LINKER,
        format!("invalid {tool_kind} configuration"),
    )
    .note(error)
    .help(format!("check the {tool_kind} and target options"))
    .finish();
    render_diagnostics_with_title(
        &format!("{tool_kind} diagnostics:"),
        std::slice::from_ref(&diagnostic),
        primary_path,
        primary_source,
    )
}

fn render_artifact_io_diagnostic(
    path: &std::path::Path,
    operation: &str,
    error: &std::io::Error,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostic = Diagnostic::user_error(
        nia_diagnostic::codes::ARTIFACT_IO,
        format!("could not {operation}"),
    )
    .note(format!("path `{}`: {error}", path.display()))
    .help("check the output directory, permissions, and available disk space")
    .finish();
    render_diagnostics_with_title(
        "driver diagnostics:",
        std::slice::from_ref(&diagnostic),
        primary_path,
        primary_source,
    )
}

/// Renders backend/codegen diagnostics with a stable title.
pub fn render_codegen_diagnostics(
    diagnostics: &[Diagnostic],
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    render_diagnostics_with_title(
        "codegen diagnostics:",
        diagnostics,
        primary_path,
        primary_source,
    )
}

/// Renders backend/codegen diagnostics as deterministic JSON.
pub fn render_codegen_diagnostics_json(diagnostics: &[Diagnostic]) -> String {
    render_diagnostics_json(diagnostics, DiagnosticReportConfig::default())
}

/// Renders any driver failure as deterministic structured JSON.
pub fn render_driver_error_json(error: &DriverError) -> String {
    render_driver_error_json_at(error, None)
}

/// Renders a driver failure as JSON, using `primary_path` for diagnostics that
/// are not wrapped in a source-manifest product.
pub fn render_driver_error_json_at(error: &DriverError, primary_path: Option<&str>) -> String {
    match error {
        DriverError::CheckDiagnostics(program) => {
            return render_program_diagnostics_json_items(
                &program.diagnostics,
                program.suppressed_downstream,
            );
        }
        DriverError::CodegenProgramDiagnostics(program) => {
            return render_program_diagnostics_json_items(
                &program.diagnostics,
                program.suppressed_downstream,
            );
        }
        DriverError::CodegenPreparationDiagnostics {
            diagnostics,
            suppressed_downstream,
        } => {
            return render_program_diagnostics_json_items(diagnostics, *suppressed_downstream);
        }
        _ => {}
    }
    let diagnostics = driver_error_diagnostics(error);
    match primary_path {
        Some(path) => {
            render_diagnostics_json_at(path, &diagnostics, DiagnosticReportConfig::default())
        }
        None => render_diagnostics_json(&diagnostics, DiagnosticReportConfig::default()),
    }
}

fn render_program_diagnostics_json_items(
    diagnostics: &[crate::ProgramDiagnostic],
    suppressed_downstream: usize,
) -> String {
    let items = diagnostics
        .iter()
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    nia_diagnostic::render_diagnostics_json_with_downstream(
        &items,
        DiagnosticReportConfig::default(),
        suppressed_downstream,
    )
}

fn driver_error_diagnostics(error: &DriverError) -> Vec<Diagnostic> {
    match error {
        DriverError::CheckDiagnostics(program) => program
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.diagnostic.clone())
            .collect(),
        DriverError::CodegenProgramDiagnostics(program) => program
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.diagnostic.clone())
            .collect(),
        DriverError::CodegenPreparationDiagnostics { diagnostics, .. } => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.diagnostic.clone())
            .collect(),
        DriverError::CodegenDiagnostics(diagnostics) => diagnostics.clone(),
        DriverError::InternalDiagnostic(diagnostic) => vec![diagnostic.clone()],
        DriverError::InvalidArtifactRequest(message) => vec![
            Diagnostic::user_error(nia_diagnostic::codes::TARGET_CONFIG, message)
                .help("choose a supported artifact mode and output combination")
                .finish(),
        ],
        DriverError::Runtime(error) => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::TARGET_CONFIG,
                "invalid runtime configuration",
            )
            .note(error.to_string())
            .help("select a runtime supported by the artifact target")
            .finish(),
        ],
        DriverError::Io {
            path,
            operation,
            error,
        } => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::ARTIFACT_IO,
                format!("could not {operation}"),
            )
            .note(format!("path `{}`: {error}", path.display()))
            .help("check the output directory, permissions, and available disk space")
            .finish(),
        ],
        DriverError::LinkerStatus {
            program,
            status,
            stderr,
        } => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                format!("linker `{program}` failed"),
            )
            .note(format!("the linker exited with status {status}"))
            .note(format!("linker output:\n{stderr}"))
            .help("inspect the linker inputs and native dependencies")
            .finish(),
        ],
        DriverError::ArchiveStatus {
            program,
            status,
            stderr,
        } => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                format!("archive tool `{program}` failed"),
            )
            .note(format!("the archive tool exited with status {status}"))
            .note(format!("archive tool output:\n{stderr}"))
            .help("inspect the archive inputs and output path")
            .finish(),
        ],
        DriverError::LinkerIo { program, error } => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                format!("could not start linker `{program}`"),
            )
            .note(format!("the operating system reported: {error}"))
            .help("install the linker, make it executable, or select a different linker")
            .finish(),
        ],
        DriverError::ArchiveIo { program, error } => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                format!("could not start archive tool `{program}`"),
            )
            .note(format!("the operating system reported: {error}"))
            .help("install the archive tool, make it executable, or select a different tool")
            .finish(),
        ],
        DriverError::LinkerConfig(error) => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                "invalid linker configuration",
            )
            .note(error.to_string())
            .help("check the linker and target options")
            .finish(),
        ],
        DriverError::ArchiveConfig(error) => vec![
            Diagnostic::user_error(
                nia_diagnostic::codes::LINKER,
                "invalid archive tool configuration",
            )
            .note(error.to_string())
            .help("check the archive tool and output options")
            .finish(),
        ],
    }
}

fn render_diagnostics_with_title(
    title: &str,
    diagnostics: &[Diagnostic],
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let report = build_diagnostic_report(diagnostics, DiagnosticReportConfig::default());
    let mut out = String::new();
    out.push_str(title);
    out.push('\n');
    let path = primary_path.unwrap_or("<codegen>");
    let source = primary_source.unwrap_or("");
    for diagnostic in report.entries() {
        out.push_str(&render_diagnostic(path, source, diagnostic));
        out.push('\n');
    }
    push_report_summary(&mut out, &report);
    out
}

/// Renders parser errors using the same diagnostic report suppression policy.
pub fn render_parse_errors(path: &str, source: &str, errors: &[crate::ParseError]) -> String {
    let diagnostics = parse_error_diagnostics(errors);
    let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());
    let mut out = String::new();
    out.push_str("parse errors:\n");
    for diagnostic in report.entries() {
        out.push_str(&render_diagnostic(path, source, diagnostic));
        out.push('\n');
    }
    push_report_summary(&mut out, &report);
    out
}

/// Renders parser errors as the machine-readable report for `path`.
pub fn render_parse_errors_json(path: &str, errors: &[crate::ParseError]) -> String {
    render_diagnostics_json_at(
        path,
        &parse_error_diagnostics(errors),
        DiagnosticReportConfig::default(),
    )
}

fn parse_error_diagnostics(errors: &[crate::ParseError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(crate::ParseError::to_diagnostic)
        .collect()
}

struct ProgramDiagnosticReportItem<'a> {
    path: &'a str,
    diagnostic: &'a Diagnostic,
}

impl DiagnosticReportItem for ProgramDiagnosticReportItem<'_> {
    fn report_diagnostic(&self) -> &Diagnostic {
        self.diagnostic
    }

    fn report_path(&self) -> Option<&str> {
        Some(self.path)
    }
}

fn push_report_summary<T: DiagnosticReportItem>(
    out: &mut String,
    report: &nia_diagnostic::DiagnosticReport<'_, T>,
) {
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
    out.push_str(&format!(
        "summary: {} {error_label}, {} {warning_label}\n",
        report.error_count(),
        report.warning_count()
    ));

    let duplicates = report.suppressed_duplicates();
    let downstream = report.suppressed_downstream();
    let by_limit = report.suppressed_by_limit();
    if duplicates == 0 && downstream == 0 && by_limit == 0 {
        return;
    }
    out.push_str(&format!(
        "note: suppressed {total} diagnostic(s) ({duplicates} duplicate(s), {downstream} downstream consequence(s), {by_limit} over limit)\n",
        total = duplicates + downstream + by_limit
    ));
}

fn diagnostic_source(
    path: &str,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    if primary_path == Some(path)
        && let Some(source) = primary_source
    {
        return source.to_string();
    }
    nia_source::read_source_text(path).unwrap_or_default()
}

fn enabled_passes_name(passes: &[&'static str]) -> String {
    if passes.is_empty() {
        "none".to_string()
    } else {
        passes.join(",")
    }
}

fn optimization_level_name(level: NiaOptimizationLevel) -> &'static str {
    match level {
        NiaOptimizationLevel::O0 => "O0",
        NiaOptimizationLevel::O1 => "O1",
        NiaOptimizationLevel::O2 => "O2",
        NiaOptimizationLevel::O3 => "O3",
        NiaOptimizationLevel::Os => "Os",
        NiaOptimizationLevel::Oz => "Oz",
    }
}

fn optimization_depth_name(depth: OptimizationDepth) -> &'static str {
    match depth {
        OptimizationDepth::Disabled => "disabled",
        OptimizationDepth::Required => "required",
        OptimizationDepth::Cheap => "cheap",
        OptimizationDepth::Full => "full",
        OptimizationDepth::Aggressive => "aggressive",
    }
}

fn inline_threshold_name(threshold: InlineThreshold) -> &'static str {
    match threshold {
        InlineThreshold::Never => "never",
        InlineThreshold::Minimal => "minimal",
        InlineThreshold::Size => "size",
        InlineThreshold::Small => "small",
        InlineThreshold::Normal => "normal",
        InlineThreshold::Aggressive => "aggressive",
    }
}

fn specialization_policy_name(policy: SpecializationPolicy) -> &'static str {
    match policy {
        SpecializationPolicy::RequiredOnly => "required-only",
        SpecializationPolicy::SizeAware => "size-aware",
        SpecializationPolicy::Normal => "normal",
        SpecializationPolicy::Aggressive => "aggressive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_diagnostic::codes;
    use nia_span::Span;

    #[test]
    fn codegen_report_prioritizes_internal_diagnostics_and_summarizes_suppressed() {
        let mut diagnostics = vec![Diagnostic::user_error_at(
            codes::PARSE,
            Span::new(0, 1),
            "duplicate user error",
        )];
        diagnostics.push(Diagnostic::user_error_at(
            codes::PARSE,
            Span::new(0, 1),
            "duplicate user error",
        ));
        diagnostics.push(Diagnostic::internal_error_at(
            codes::ICE,
            Span::new(2, 3),
            "internal error",
        ));
        for index in 0..25 {
            diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                Span::new(10 + index, 11 + index),
                format!("user error {index}"),
            ));
        }

        let rendered = render_codegen_diagnostics(&diagnostics, Some("main.nia"), Some("abc"));

        let internal = rendered
            .find("error internal[I0001]")
            .expect("internal diagnostic");
        let user = rendered.find("error[E0101]").expect("user diagnostic");
        assert!(internal < user, "{rendered}");
        assert!(rendered.contains("note: suppressed"), "{rendered}");
        assert!(rendered.contains("1 duplicate(s)"), "{rendered}");
        assert!(
            rendered.contains("summary: 20 errors, 0 warnings"),
            "{rendered}"
        );
    }

    #[test]
    fn parse_report_uses_parse_error_code() {
        let errors = vec![crate::ParseError {
            span: Span::new(0, 1),
            kind: crate::ParseErrorKind::Grammar,
            message: "bad token".to_string(),
            node_key: None,
        }];

        let rendered = render_parse_errors("main.nia", "?", &errors);

        assert!(rendered.contains("error[E0101]"), "{rendered}");
    }

    #[test]
    fn parse_reports_share_rule_help_across_text_and_json() {
        let source = "fn main() { let value = 1 }";
        let (_module, errors) = nia_parser::parse_module(source);
        let help = "add the missing `;` to terminate this declaration or statement";

        let text = render_parse_errors("main.nia", source, &errors);
        let json = render_parse_errors_json("main.nia", &errors);

        assert!(text.contains("error[E0101]"), "{text}");
        assert!(text.contains(help), "{text}");
        assert!(json.contains(r#""code":"E0101""#), "{json}");
        assert!(json.contains(r#""path":"main.nia""#), "{json}");
        assert!(json.contains(help), "{json}");
    }

    #[test]
    #[cfg(unix)]
    fn external_tool_report_keeps_bounded_stderr_as_structured_note() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 23"])
            .status()
            .expect("run shell");
        let rendered = render_driver_error(
            &DriverError::LinkerStatus {
                program: "ld".to_string(),
                status,
                stderr: "undefined reference to `missing`".to_string(),
            },
            Some("main.nia"),
            Some("fn main() () {}"),
        );
        assert!(rendered.contains("error[E0701]"), "{rendered}");
        assert!(
            rendered.contains("undefined reference to `missing`"),
            "{rendered}"
        );
        assert!(rendered.contains("inspect the linker inputs"), "{rendered}");
    }

    #[test]
    #[cfg(unix)]
    fn archive_tool_report_keeps_archive_ownership() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 17"])
            .status()
            .expect("run shell");
        let rendered = render_driver_error(
            &DriverError::ArchiveStatus {
                program: "ar".to_string(),
                status,
                stderr: "invalid archive member".to_string(),
            },
            Some("main.nia"),
            Some("fn main() () {}"),
        );
        assert!(
            rendered.starts_with("archive tool diagnostics:"),
            "{rendered}"
        );
        assert!(rendered.contains("archive tool `ar` failed"), "{rendered}");
        assert!(rendered.contains("archive tool output:"), "{rendered}");
        assert!(!rendered.contains("linker diagnostics:"), "{rendered}");
    }

    #[test]
    fn codegen_json_report_uses_the_shared_diagnostic_contract() {
        let diagnostics = vec![Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            Span::new(2, 4),
            "type mismatch",
        )];
        let json = render_codegen_diagnostics_json(&diagnostics);
        assert!(json.contains("\"code\":\"E0301\""), "{json}");
        assert!(json.contains("\"start\":2,\"end\":4"), "{json}");
        assert!(json.contains("\"suppressed\""), "{json}");
    }

    #[test]
    #[cfg(unix)]
    fn driver_error_json_does_not_embed_terminal_formatting() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 9"])
            .status()
            .expect("run shell");
        let json = render_driver_error_json(&DriverError::LinkerStatus {
            program: "ld".to_string(),
            status,
            stderr: "missing symbol".to_string(),
        });
        assert!(json.starts_with('{'), "{json}");
        assert!(json.contains("\"code\":\"E0701\""), "{json}");
        assert!(!json.contains("error[E0701]"), "{json}");
    }

    #[test]
    #[cfg(unix)]
    fn archive_tool_json_keeps_archive_ownership() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 17"])
            .status()
            .expect("run shell");
        let json = render_driver_error_json(&DriverError::ArchiveStatus {
            program: "ar".to_string(),
            status,
            stderr: "invalid archive member".to_string(),
        });
        assert!(json.contains("archive tool `ar` failed"), "{json}");
        assert!(json.contains("archive tool output:"), "{json}");
        assert!(!json.contains("external tool `ar` failed"), "{json}");
    }

    #[test]
    fn raw_codegen_json_uses_the_callers_source_path() {
        let diagnostics = vec![Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            Span::new(1, 2),
            "invalid expression",
        )];
        let json = render_driver_error_json_at(
            &DriverError::CodegenDiagnostics(diagnostics),
            Some("main.nia"),
        );
        assert!(json.contains("\"path\":\"main.nia\""), "{json}");
    }
}
