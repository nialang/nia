// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_ids::ModuleId;
use nia_opt::OptimizationPolicy;

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmCodegenOutput {
    pub modules: Vec<LlvmModuleOutput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmModuleOutput {
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
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LlvmCodegenOptions {
    pub root_module: Option<ModuleId>,
    pub hosted_entry: bool,
    pub optimization: OptimizationPolicy,
}
