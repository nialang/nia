// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BackendOptimizationChange, CheckedProgram, DriverError, NiaOptimizationLevel};
use nia_diagnostic::{Diagnostic, render_diagnostic};
use nia_opt::{InlineThreshold, OptimizationDepth, SpecializationPolicy};
use std::fs;

pub fn optimization_report(program: &CheckedProgram) -> String {
    let mut out = optimization_report_lines(program).join("\n");
    out.push('\n');
    out
}

pub fn optimization_report_lines(program: &CheckedProgram) -> Vec<String> {
    let report = &program.backend_lowering.optimization_report;
    let policy = program.optimization;
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
                    function.module_id.0, function.def_id.0, instance, pass, type_arg_count
                ));
            }
            BackendOptimizationChange::Global { global, pass, .. } => {
                lines.push(format!(
                    "  m{}::d{} global {}",
                    global.module_id.0, global.def_id.0, pass
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
    let mut out = String::new();
    out.push_str("diagnostics:\n");
    for diagnostic in &program.diagnostics {
        let source = diagnostic_source(diagnostic.path.as_str(), primary_path, primary_source);
        out.push_str(&render_diagnostic(
            diagnostic.path.as_str(),
            &source,
            &diagnostic.diagnostic,
        ));
        out.push('\n');
    }
    out
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
        DriverError::CodegenDiagnostics(diagnostics) => {
            render_codegen_diagnostics(diagnostics, primary_path, primary_source)
        }
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
    let mut out = String::new();
    out.push_str("codegen diagnostics:\n");
    let path = primary_path.unwrap_or("<codegen>");
    let source = primary_source.unwrap_or("");
    for diagnostic in diagnostics {
        out.push_str(&render_diagnostic(path, source, diagnostic));
        out.push('\n');
    }
    out
}

pub fn render_parse_errors(path: &str, source: &str, errors: &[crate::ParseError]) -> String {
    let mut out = String::new();
    out.push_str("parse errors:\n");
    for error in errors {
        let diagnostic = Diagnostic::user_error_at("E0103", error.span, error.message.clone());
        out.push_str(&render_diagnostic(path, source, &diagnostic));
        out.push('\n');
    }
    out
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
