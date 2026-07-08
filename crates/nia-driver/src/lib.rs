// SPDX-License-Identifier: GPL-3.0-or-later
mod executable_cache;
mod inspect;
mod pipeline;
mod report;

pub use executable_cache::{
    ExecutableArtifactCacheEntry, ExecutableArtifactCacheInput, ExecutableArtifactCacheRequest,
    ExecutableArtifactCacheSnapshot, executable_artifact_cache_entry,
    publish_executable_artifact_cache, restore_executable_artifact_cache,
};
pub use inspect::{AstInspection, TokensInspection, ast_inspection, tokens_inspection};
pub use nia_compiler_query::{
    BackendOptimizationChange, CheckedModule, CheckedProgram, CodegenProgram, LoadedModule,
    LoadedProgram, ProgramDiagnostic, TimingMode,
};
pub use nia_imports::{ENTRY_MODULE_MAP_NAME, ModuleMap, PACKAGE_MODULE_MAP_NAME};
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
    render_codegen_program_warnings, render_driver_error, render_parse_errors,
    render_program_diagnostics, render_program_warnings,
};

#[cfg(test)]
mod tests;
