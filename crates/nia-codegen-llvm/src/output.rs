// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey};
use nia_diagnostic::Diagnostic;
use nia_opt::OptimizationPolicy;

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmCodegenOutput {
    pub modules: Vec<LlvmModuleOutput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmModuleOutput {
    pub unit: CodegenUnitId,
    pub key: CodegenUnitKey,
    pub fingerprint: CodegenUnitFingerprint,
    pub name: String,
    pub ir: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmObjectOutput {
    pub modules: Vec<LlvmObjectModuleOutput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmObjectModuleOutput {
    pub unit: CodegenUnitId,
    pub key: CodegenUnitKey,
    pub fingerprint: CodegenUnitFingerprint,
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LlvmCodegenOptions {
    pub optimization: OptimizationPolicy,
    pub timings: nia_timing::TimingMode,
}
