// SPDX-License-Identifier: GPL-3.0-or-later
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;

use module_codegen::ModuleCodegen;
use nia_backend_ir::BackendProgram;
use nia_diagnostic::Diagnostic;
use nia_llvm::{Context, target::TargetMachine};
use nia_span::Span;
pub use output::{
    LlvmCodegenOptions, LlvmCodegenOutput, LlvmModuleOutput, LlvmObjectModuleOutput,
    LlvmObjectOutput,
};
use program_index::ProgramIndex;

pub fn emit_llvm_ir(program: &BackendProgram) -> LlvmCodegenOutput {
    emit_llvm_ir_with_options(program, LlvmCodegenOptions::default())
}

pub fn emit_llvm_ir_with_options(
    program: &BackendProgram,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    let index = ProgramIndex::new(program);
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    for module in &program.modules {
        let context = Context::create();
        let mut codegen = ModuleCodegen::new(&context, module, &index, options);
        match codegen.emit_ir() {
            Ok(ir) => outputs.push(LlvmModuleOutput {
                name: module.name.clone(),
                ir,
            }),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    LlvmCodegenOutput {
        modules: outputs,
        diagnostics,
    }
}

pub fn emit_native_objects(
    program: &BackendProgram,
    options: LlvmCodegenOptions,
) -> LlvmObjectOutput {
    let target = match TargetMachine::native() {
        Ok(target) => target,
        Err(message) => {
            return LlvmObjectOutput {
                modules: Vec::new(),
                diagnostics: vec![Diagnostic::error(Span::default(), message)],
            };
        }
    };
    let index = ProgramIndex::new(program);
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    for module in &program.modules {
        let context = Context::create();
        let mut codegen = ModuleCodegen::new(&context, module, &index, options);
        target.configure_module(&codegen.module);
        match codegen.emit_object(&target) {
            Ok(bytes) => outputs.push(LlvmObjectModuleOutput {
                name: module.name.clone(),
                bytes,
            }),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    LlvmObjectOutput {
        modules: outputs,
        diagnostics,
    }
}

#[cfg(test)]
mod tests;
