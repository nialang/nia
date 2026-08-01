// SPDX-License-Identifier: GPL-3.0-or-later
mod executable_cache;
mod inspect;
mod object_cache;
mod pipeline;
mod report;

pub use inspect::{AstInspection, TokensInspection, ast_inspection, tokens_inspection};
pub use nia_compiler_query::{
    BackendOptimizationChange, BackendOptimizationReport, CheckedModule, CheckedProgram,
    CodegenProgram, LoadedModule, LoadedProgram, ProgramDiagnostic, TimingMode,
};
pub use nia_imports::{ENTRY_MODULE_MAP_NAME, ModuleMap, PACKAGE_MODULE_MAP_NAME};
pub use nia_loader_query::{SourceInput, SourceInputContent, SourceInputManifest};
pub use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
pub use nia_parser::ParseError;
pub use nia_source::SourcePath;
pub use nia_toolchain::{
    ToolchainIdentity, ToolchainLayout, ToolchainLayoutError, ToolchainLayoutRequest,
};
pub use pipeline::{
    CheckRequest, CheckedProgramWithSourceManifest, Driver, DriverConfig, DriverError,
    DriverOutput, EmitLlvmRequest, EmitObjectRequest, ExecutableArtifact,
    ExecutableCacheEnvironment, ExecutableCacheReference, ExecutableCacheRestore,
    LinkExecutableRequest, LinkedExecutableWithSourceManifest, LlvmIrArtifact, ObjectArtifact,
    ObjectOutput, Runtime, WriteObjectRequest, WrittenObjectArtifact,
};
pub use report::{
    llvm_ir_optimization_report, object_optimization_report, optimization_report,
    optimization_report_from_parts, optimization_report_lines, render_codegen_diagnostics,
    render_codegen_program_warnings, render_driver_error, render_executable_warnings,
    render_llvm_ir_warnings, render_object_warnings, render_parse_errors,
    render_program_diagnostics, render_program_warnings,
};

#[cfg(test)]
mod tests;
