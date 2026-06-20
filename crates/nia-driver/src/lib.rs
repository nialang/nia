// SPDX-License-Identifier: GPL-3.0-or-later
mod inspect;
mod pipeline;
mod report;

pub use inspect::{AstInspection, TokensInspection, ast_inspection, tokens_inspection};
pub use nia_compiler_query::{
    BackendOptimizationChange, CheckedModule, CheckedProgram, LoadedModule, LoadedProgram,
    ProgramDiagnostic, TimingMode,
};
pub use nia_imports::{
    BUILTIN_MODULE_MAP_NAME, ModuleMap, PACKAGE_MODULE_MAP_NAME, ROOT_MODULE_MAP_NAME,
};
pub use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
pub use nia_parser::ParseError;
pub use nia_source::SourcePath;
pub use pipeline::{
    CheckRequest, Driver, DriverConfig, DriverError, DriverOutput, EmitLlvmRequest,
    EmitObjectRequest, ExecutableArtifact, LinkExecutableRequest, LlvmIrArtifact, ObjectArtifact,
    ObjectOutput, Runtime, WriteObjectRequest, WrittenObjectArtifact,
};
pub use report::{
    optimization_report, optimization_report_lines, render_codegen_diagnostics,
    render_driver_error, render_parse_errors, render_program_diagnostics,
};

#[cfg(test)]
mod tests;
