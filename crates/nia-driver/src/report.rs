// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    BackendOptimizationChange, CheckedProgram, CodegenProgram, DriverError, LlvmIrArtifact,
    NiaOptimizationLevel, ObjectArtifact,
};
use nia_diagnostic::{
    Diagnostic, DiagnosticReportConfig, DiagnosticReportItem, build_diagnostic_report,
    render_diagnostic,
};
use nia_opt::{InlineThreshold, OptimizationDepth, SpecializationPolicy};
use std::fs;

pub fn optimization_report(program: &CodegenProgram) -> String {
    let mut out = optimization_report_lines_from_parts(
        program.optimization,
        &program.backend_lowering.optimization_report,
    )
    .join("\n");
    out.push('\n');
    out
}

pub fn optimization_report_lines(program: &CodegenProgram) -> Vec<String> {
    optimization_report_lines_from_parts(
        program.optimization,
        &program.backend_lowering.optimization_report,
    )
}

pub fn llvm_ir_optimization_report(artifact: &LlvmIrArtifact) -> String {
    let mut out =
        optimization_report_lines_from_parts(artifact.optimization, &artifact.optimization_report)
            .join("\n");
    out.push('\n');
    out
}

pub fn object_optimization_report(artifact: &ObjectArtifact) -> String {
    let mut out =
        optimization_report_lines_from_parts(artifact.optimization, &artifact.optimization_report)
            .join("\n");
    out.push('\n');
    out
}

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

pub fn render_program_diagnostics(
    program: &CheckedProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    render_program_diagnostic_items(&program.diagnostics, primary_path, primary_source)
}

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
    render_program_diagnostic_items(&diagnostics, primary_path, primary_source)
}

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
    render_program_diagnostic_items(&diagnostics, primary_path, primary_source)
}

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
    render_program_diagnostic_items(&diagnostics, primary_path, primary_source)
}

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
    render_program_diagnostic_items(&diagnostics, primary_path, primary_source)
}

fn render_program_diagnostic_items(
    diagnostics: &[crate::ProgramDiagnostic],
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    let diagnostics = diagnostics
        .iter()
        .map(|diagnostic| ProgramDiagnosticReportItem {
            path: diagnostic.path.as_str(),
            diagnostic: &diagnostic.diagnostic,
        })
        .collect::<Vec<_>>();
    let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());
    let mut out = String::new();
    out.push_str("diagnostics:\n");
    for entry in report.entries() {
        let source = diagnostic_source(entry.path, primary_path, primary_source);
        out.push_str(&render_diagnostic(entry.path, &source, entry.diagnostic));
        out.push('\n');
    }
    push_suppressed_summary(
        &mut out,
        report.suppressed_duplicates(),
        report.suppressed_by_limit(),
    );
    out
}

pub fn render_codegen_program_diagnostics(
    program: &CodegenProgram,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    render_program_diagnostic_items(&program.diagnostics, primary_path, primary_source)
}

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
    render_program_diagnostic_items(&diagnostics, primary_path, primary_source)
}

pub fn render_driver_error(
    error: &DriverError,
    primary_path: Option<&str>,
    primary_source: Option<&str>,
) -> String {
    match error {
        DriverError::CheckDiagnostics(program) => {
            render_program_diagnostics(program, primary_path, primary_source)
        }
        DriverError::CodegenProgramDiagnostics(program) => {
            render_codegen_program_diagnostics(program, primary_path, primary_source)
        }
        DriverError::CodegenPreparationDiagnostics(diagnostics) => {
            render_program_diagnostic_items(diagnostics, primary_path, primary_source)
        }
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
            let mut out = String::new();
            out.push_str(message);
            out.push('\n');
            out
        }
        DriverError::Io {
            path,
            operation: _,
            error,
        } => {
            format!("failed to write `{}`: {error}\n", path.display())
        }
        DriverError::LinkerStatus { program, status } => {
            format!("linker `{program}` failed with status {status}\n")
        }
        DriverError::LinkerIo { program, error } => {
            format!("failed to run linker `{program}`: {error}\n")
        }
        DriverError::LinkerConfig(error) => format!("invalid linker configuration: {error}\n"),
    }
}

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
    push_suppressed_summary(
        &mut out,
        report.suppressed_duplicates(),
        report.suppressed_by_limit(),
    );
    out
}

pub fn render_parse_errors(path: &str, source: &str, errors: &[crate::ParseError]) -> String {
    let diagnostics = errors
        .iter()
        .map(|error| {
            Diagnostic::user_error_at(
                nia_diagnostic::codes::PARSE,
                error.span,
                error.message.clone(),
            )
        })
        .collect::<Vec<_>>();
    let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());
    let mut out = String::new();
    out.push_str("parse errors:\n");
    for diagnostic in report.entries() {
        out.push_str(&render_diagnostic(path, source, diagnostic));
        out.push('\n');
    }
    push_suppressed_summary(
        &mut out,
        report.suppressed_duplicates(),
        report.suppressed_by_limit(),
    );
    out
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

fn push_suppressed_summary(out: &mut String, duplicates: usize, by_limit: usize) {
    if duplicates == 0 && by_limit == 0 {
        return;
    }
    out.push_str(&format!(
        "note: suppressed {total} diagnostic(s)",
        total = duplicates + by_limit
    ));
    if duplicates > 0 || by_limit > 0 {
        out.push_str(&format!(
            " ({duplicates} duplicate(s), {by_limit} over limit)"
        ));
    }
    out.push('\n');
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
    fs::read_to_string(path).unwrap_or_default()
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
    }

    #[test]
    fn parse_report_uses_parse_error_code() {
        let errors = vec![crate::ParseError {
            span: Span::new(0, 1),
            message: "bad token".to_string(),
            node_key: None,
        }];

        let rendered = render_parse_errors("main.nia", "?", &errors);

        assert!(rendered.contains("error[E0101]"), "{rendered}");
    }
}
