// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checked_program_smoke_matrix_emits_llvm_ir() {
    for case in emit_smoke_cases() {
        let root = temp_dir(&format!("checked_program_smoke_matrix_{}", case.name));
        write_smoke_case(&root, case);
        let checked =
            nia_driver::check_program(root.join(case.root).to_string_lossy().into_owned());
        assert!(
            checked.diagnostics.is_empty(),
            "{} check diagnostics: {:?}",
            case.name,
            checked.diagnostics
        );
        let output = emit_llvm_ir(&checked.backend_lowering.program);
        assert!(
            output.diagnostics.is_empty(),
            "{} codegen diagnostics: {:?}",
            case.name,
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            checked.backend_lowering.program.modules.len(),
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
fn checked_program_emits_llvm_ir_with_each_nia_optimization_level() {
    let root = temp_dir("checked_program_emits_llvm_ir_with_each_nia_optimization_level");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
let zeroes: [4]i32 = [0; 4];

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
        nia_driver::NiaOptimizationLevel::O0,
        nia_driver::NiaOptimizationLevel::O1,
        nia_driver::NiaOptimizationLevel::O2,
        nia_driver::NiaOptimizationLevel::O3,
        nia_driver::NiaOptimizationLevel::Os,
        nia_driver::NiaOptimizationLevel::Oz,
    ] {
        let checked =
            nia_driver::check_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(
            checked.diagnostics.is_empty(),
            "{level:?} check diagnostics: {:?}",
            checked.diagnostics
        );
        assert_eq!(checked.optimization, level.policy(), "{level:?}");

        let output = emit_llvm_ir_with_options(
            &checked.backend_lowering.program,
            LlvmCodegenOptions {
                root_module: Some(checked.graph.root()),
                hosted_entry: false,
                optimization: checked.optimization,
            },
        );
        assert!(
            output.diagnostics.is_empty(),
            "{level:?} codegen diagnostics: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            checked.backend_lowering.program.modules.len(),
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
