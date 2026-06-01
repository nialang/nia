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
