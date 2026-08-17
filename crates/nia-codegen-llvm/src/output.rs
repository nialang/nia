// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{
    CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInputs,
};
use nia_diagnostic::Diagnostic;
use nia_opt::OptimizationPolicy;

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
