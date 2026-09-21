// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{
    CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInputs,
};
use nia_diagnostic::Diagnostic;
use nia_llvm::target::TargetMachineIdentity;
use nia_opt::OptimizationPolicy;
use nia_source::SourceIdentity;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
/// Textual LLVM output and diagnostics for a complete codegen request.
///
/// Successful units remain available when another independent unit fails.
pub struct LlvmCodegenOutput {
    /// Successfully validated and emitted codegen units.
    pub modules: Vec<LlvmModuleOutput>,
    /// Validation, LLVM construction, or target failures from omitted units.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
/// Textual LLVM IR for one stable incremental codegen unit.
pub struct LlvmModuleOutput {
    /// Per-build numeric identity used by the backend partition plan.
    pub unit: CodegenUnitId,
    /// Stable identity used for deterministic ordering and cache ownership.
    pub key: CodegenUnitKey,
    /// Complete content fingerprint for the emitted unit.
    pub fingerprint: CodegenUnitFingerprint,
    /// Human-readable LLVM module name.
    pub name: String,
    /// Verified textual LLVM IR.
    pub ir: String,
}

#[derive(Debug, Clone, PartialEq)]
/// LLVM pre-link modules ready for one whole-program coordination run.
pub struct LlvmLtoModuleOutput {
    /// Pre-link policy used to produce every module in this linkage unit.
    pub pre_link: LtoPreLinkConfig,
    /// Exact target identity shared by every successfully emitted module.
    pub target: Option<TargetMachineIdentity>,
    /// Deterministically ordered LTO pre-link inputs.
    pub modules: Vec<LtoModule>,
    /// External definitions that must remain visible to regular linker inputs.
    pub linker_visible_symbols: Vec<String>,
    /// Validation, LLVM construction, or target failures from omitted units.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
/// Pre-link bitcode for one stable incremental codegen unit.
pub struct LtoModule {
    /// Per-build numeric identity used by the backend partition plan.
    pub unit: CodegenUnitId,
    /// Stable identity used for deterministic ordering and cache ownership.
    pub key: CodegenUnitKey,
    /// Complete content fingerprint for this pre-link product.
    pub fingerprint: CodegenUnitFingerprint,
    /// Human-readable source module name.
    pub name: String,
    /// Unique stable identifier used by LLVM's combined summary index.
    pub module_identifier: String,
    /// Target-configured bitcode for the selected LTO pipeline.
    pub bitcode: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LLVM whole-program optimization model selected for final linking.
pub enum LtoMode {
    /// Summary-index analysis with parallel per-module importing backends.
    Thin,
    /// Monolithic IR merge and whole-program optimization.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Policy that determines the contents and cache identity of LTO pre-link bitcode.
pub struct LtoPreLinkConfig {
    /// Whole-program optimization model used after pre-link emission.
    pub mode: LtoMode,
    /// Exclude assumptions about hosted target-library functions.
    pub freestanding: bool,
}

#[derive(Debug, Clone, Copy)]
/// Whole-program policy for coordinating emitted ThinLTO modules.
pub struct ThinLtoCodegenConfig<'a> {
    /// Maximum number of in-process ThinLTO backend workers.
    pub parallelism: usize,
    /// Definitions that must remain visible to regular native linker inputs.
    pub preserved_symbols: &'a [&'a str],
    /// Release-isolated persistent cache for LLVM ThinLTO backend objects.
    pub backend_cache_directory: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy)]
/// Whole-program policy for coordinating full-LTO modules.
pub struct FullLtoCodegenConfig<'a> {
    /// Stable logical entry source owning the final linkage unit.
    pub linkage_source_identity: &'a SourceIdentity,
    /// Number of native partitions after monolithic optimization.
    pub parallelism: usize,
    /// Definitions that must remain visible to regular native linker inputs.
    pub preserved_symbols: &'a [&'a str],
}

#[derive(Debug, Clone, PartialEq)]
/// Native objects ready for linking plus diagnostics for failed units.
pub struct LlvmObjectOutput {
    /// Deterministically ordered fresh or reused linker inputs.
    pub link_inputs: IncrementalLinkInputs<NativeObject>,
    /// Validation, cache, LLVM construction, or target failures.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
/// Freshly emitted native object bytes for one codegen unit.
pub struct NativeObject {
    /// Per-build numeric identity used by the backend partition plan.
    pub unit: CodegenUnitId,
    /// Human-readable object stem derived from the stable unit key.
    pub name: String,
    /// Target object-file bytes copied out of LLVM's memory buffer.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Policy inputs that affect LLVM output and incremental object identity.
pub struct LlvmCodegenOptions {
    /// Language and backend optimization policy.
    pub optimization: OptimizationPolicy,
    /// Timing and counter emission mode.
    pub timings: nia_timing::TimingMode,
    /// Exact toolchain identity included in target fingerprints.
    pub toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
}

impl Default for LlvmCodegenOptions {
    fn default() -> Self {
        Self {
            optimization: OptimizationPolicy::default(),
            timings: nia_timing::TimingMode::Off,
            toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint::current(),
        }
    }
}
