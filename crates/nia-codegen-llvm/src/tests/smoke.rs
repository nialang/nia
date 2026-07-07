// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn codegen_program_smoke_matrix_emits_llvm_ir() {
    for case in emit_smoke_cases() {
        let root = temp_dir(&format!("codegen_program_smoke_matrix_{}", case.name));
        write_smoke_case(&root, case);
        let codegen = codegen_program(root.join(case.root).to_string_lossy().into_owned());
        assert!(
            codegen.diagnostics.is_empty(),
            "{} codegen diagnostics: {:?}",
            case.name,
            codegen.diagnostics
        );
        let output = emit_llvm_ir(&codegen.backend_lowering.program);
        assert!(
            output.diagnostics.is_empty(),
            "{} codegen diagnostics: {:?}",
            case.name,
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            codegen.backend_lowering.program.modules.len(),
            "{} should emit one LLVM module per backend module",
            case.name
        );
        assert!(
            output
                .modules
                .iter()
                .all(|module| module.ir.contains("source_filename")),
            "{} emitted empty or malformed IR: {:?}",
            case.name,
            output
                .modules
                .iter()
                .map(|module| (&module.name, module.ir.len()))
                .collect::<Vec<_>>()
        );
        let joined_ir = output
            .modules
            .iter()
            .map(|module| module.ir.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined_ir.contains("define"),
            "{} should emit at least one function definition",
            case.name
        );
    }
}

#[test]
fn codegen_program_emits_llvm_ir_with_each_nia_optimization_level() {
    let root = temp_dir("codegen_program_emits_llvm_ir_with_each_nia_optimization_level");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
static zeroes: [4]i32 = [0; 4];

fn answer() i32 {
    42
}

fn main() i32 {
    answer() + zeroes[0]
}
"#,
    )
    .expect("write test source");

    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::O3,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let codegen = codegen_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(
            codegen.diagnostics.is_empty(),
            "{level:?} codegen diagnostics: {:?}",
            codegen.diagnostics
        );
        assert_eq!(codegen.optimization, level.policy(), "{level:?}");

        let output = emit_llvm_ir_with_options(
            &codegen.backend_lowering.program,
            LlvmCodegenOptions {
                optimization: codegen.optimization,
                ..LlvmCodegenOptions::default()
            },
        );
        assert!(
            output.diagnostics.is_empty(),
            "{level:?} codegen diagnostics: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            codegen.backend_lowering.program.modules.len(),
            "{level:?}"
        );
        let joined_ir = output
            .modules
            .iter()
            .map(|module| module.ir.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined_ir.contains("define i32 @"), "{level:?}: {joined_ir}");
    }
}
