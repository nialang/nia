// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler orchestration, artifact production, and diagnostic rendering.
//!
//! `nia-driver` owns request sequencing and cache/artifact boundaries. Semantic
//! meaning remains in query and analysis crates; the driver carries source
//! manifests through checking, code generation, linking, and publication so a
//! result cannot be associated with a different source closure.
//!
//! The driver loads sources, builds the using graph, computes public surfaces,
//! and schedules whole-program checking and code generation by requesting
//! query products. The using graph is acyclic; semantic cycles inside loaded
//! modules are diagnosed by the query or crate that owns the affected
//! construct. Its object/LTO, archive, and linked-executable caches hold only
//! validated, disposable products.
//!
//! Preparation errors and artifacts keep the retained diagnostic source
//! snapshots produced by compiler queries. When rendering, a retained snapshot
//! takes precedence over a caller-supplied source and over disk reads, and an
//! explicit missing-source entry prohibits reading a later file at that path.
//! Linker and archiver failures keep both captured output streams and use the
//! same diagnostic constructor for text and JSON output.
//!
//! Executable codegen has three final-link policies: no LTO (the default),
//! ThinLTO, and full LTO. LTO is executable-only, because object sets and
//! static archives do not own a complete final linkage unit. The driver
//! prepares the ThinLTO backend cache namespace beneath the invocation cache
//! root before passing it to LLVM.

fn stream_chunk_len(remaining: u64, buffer_len: usize) -> usize {
    usize::try_from(remaining).map_or(buffer_len, |remaining| remaining.min(buffer_len))
}

mod archive_cache;
mod cache_fs;
mod executable_cache;
mod inspect;
mod object_cache;
mod pipeline;
mod report;

pub use inspect::{AstInspection, TokensInspection, ast_inspection, tokens_inspection};
pub use nia_compiler_query::{
    BackendOptimizationChange, BackendOptimizationReport, CheckedModule, CheckedProgram,
    CodegenProgram, DiagnosticSources, LoadedModule, LoadedProgram, ProgramDiagnostic, TimingMode,
};
pub use nia_imports::{ENTRY_MODULE_MAP_NAME, ModuleMap, PACKAGE_MODULE_MAP_NAME};
pub use nia_loader_query::{SourceInput, SourceInputContent, SourceInputManifest};
pub use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
pub use nia_package_metadata::PackageId;
pub use nia_source::SourcePath;
pub use nia_syntax::{ParseError, ParseErrorKind};
pub use nia_target_config::{BuildProfile, CompilationMode};
pub use nia_toolchain::{
    RuntimeSpec, RuntimeSpecError, ToolchainIdentity, ToolchainLayout, ToolchainLayoutError,
    ToolchainLayoutRequest,
};
pub use pipeline::{
    CheckRequest, CheckedProgramWithSourceManifest, Driver, DriverConfig, DriverError,
    DriverOutput, EmitLlvmRequest, EmitObjectRequest, ExecutableArtifact,
    ExecutableCacheEnvironment, ExecutableCacheReference, ExecutableCacheRestore,
    LinkExecutableRequest, LinkTimeOptimization, LinkedExecutableWithSourceManifest,
    LlvmIrArtifact, ObjectArtifact, ObjectOutput, StaticArchiveArtifact,
    StaticArchiveCacheEnvironment, StaticArchiveCacheReference, StaticArchiveCacheRestore,
    WriteObjectRequest, WrittenObjectArtifact,
};
pub use report::{
    llvm_ir_optimization_report, object_optimization_report, optimization_report,
    optimization_report_from_parts, optimization_report_lines, render_artifact_warnings_json,
    render_codegen_diagnostics, render_codegen_diagnostics_json, render_codegen_program_warnings,
    render_codegen_program_warnings_json, render_driver_error, render_driver_error_json,
    render_driver_error_json_at, render_executable_warnings, render_llvm_ir_warnings,
    render_object_warnings, render_parse_errors, render_parse_errors_json,
    render_program_diagnostics, render_program_diagnostics_json, render_program_warnings,
    render_program_warnings_json,
};

#[cfg(test)]
mod tests;
