// SPDX-License-Identifier: GPL-3.0-or-later
mod backend_validate;
mod function_codegen;
mod literals;
mod module_codegen;
mod output;
mod program_index;

use backend_validate::validate_backend_program;
use module_codegen::ModuleCodegen;
use nia_backend_ir::BackendProgram;
use nia_llvm::{Context, OptimizationLevel as LlvmOptimizationLevel, target::TargetMachine};
use nia_opt::OptimizationLevel;
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
    catch_llvm_codegen_ice(|| emit_llvm_ir_with_options_inner(program, options))
}

fn emit_llvm_ir_with_options_inner(
    program: &BackendProgram,
    options: LlvmCodegenOptions,
) -> LlvmCodegenOutput {
    let index = ProgramIndex::new(program);
    let validation_diagnostics = validate_backend_program(program, &index);
    if !validation_diagnostics.is_empty() {
        return LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    for module in &program.modules {
        let context = Context::create();
        let mut codegen = match ModuleCodegen::new(&context, module, &index, options) {
            Ok(codegen) => codegen,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
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
    catch_llvm_object_ice(|| emit_native_objects_inner(program, options))
}

fn emit_native_objects_inner(
    program: &BackendProgram,
    options: LlvmCodegenOptions,
) -> LlvmObjectOutput {
    let target = match TargetMachine::native_with_opt_level(llvm_optimization_level(
        options.optimization.level,
    )) {
        Ok(target) => target,
        Err(error) => {
            return LlvmObjectOutput {
                modules: Vec::new(),
                diagnostics: vec![error.diagnostic()],
            };
        }
    };
    let index = ProgramIndex::new(program);
    let validation_diagnostics = validate_backend_program(program, &index);
    if !validation_diagnostics.is_empty() {
        return LlvmObjectOutput {
            modules: Vec::new(),
            diagnostics: validation_diagnostics,
        };
    }
    let mut outputs = Vec::new();
    let mut diagnostics = Vec::new();
    for module in &program.modules {
        let context = Context::create();
        let mut codegen = match ModuleCodegen::new(&context, module, &index, options) {
            Ok(codegen) => codegen,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
        if let Err(error) = target.configure_module(&codegen.module) {
            diagnostics.push(error.diagnostic());
            continue;
        }
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

fn catch_llvm_codegen_ice(f: impl FnOnce() -> LlvmCodegenOutput) -> LlvmCodegenOutput {
    match nia_ice::catch_ice(f) {
        Ok(output) => output,
        Err(ice) => LlvmCodegenOutput {
            modules: Vec::new(),
            diagnostics: vec![ice.diagnostic()],
        },
    }
}

fn catch_llvm_object_ice(f: impl FnOnce() -> LlvmObjectOutput) -> LlvmObjectOutput {
    match nia_ice::catch_ice(f) {
        Ok(output) => output,
        Err(ice) => LlvmObjectOutput {
            modules: Vec::new(),
            diagnostics: vec![ice.diagnostic()],
        },
    }
}

fn llvm_optimization_level(level: OptimizationLevel) -> LlvmOptimizationLevel {
    match level {
        OptimizationLevel::O0 => LlvmOptimizationLevel::None,
        OptimizationLevel::O1 => LlvmOptimizationLevel::Less,
        OptimizationLevel::O2 | OptimizationLevel::Os => LlvmOptimizationLevel::Default,
        OptimizationLevel::O3 => LlvmOptimizationLevel::Aggressive,
        OptimizationLevel::Oz => LlvmOptimizationLevel::Less,
    }
}

#[cfg(test)]
mod tests;
