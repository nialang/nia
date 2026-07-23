// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn large_source_module_emits_multiple_non_overlapping_partitions() {
    let root = temp_dir("large_source_module_emits_multiple_non_overlapping_partitions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn helper0() i32 { 0 }
fn helper1() i32 { 1 }
fn helper2() i32 { 2 }
fn helper3() i32 { 3 }
fn helper4() i32 { 4 }
fn helper5() i32 { 5 }
fn helper6() i32 { 6 }

fn main() i32 {
    helper0() + helper1() + helper2() + helper3()
        + helper4() + helper5() + helper6()
}
"#,
    )
    .expect("write partition source");
    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let source_identity = nia_source::SourceIdentity::new(main.to_string_lossy());
    let source_partitions = codegen
        .backend_lowering
        .codegen_partitions
        .partitions()
        .iter()
        .filter(|partition| {
            matches!(
                &partition.key,
                CodegenUnitKey::SourceModule {
                    source_identity: identity,
                    ..
                } if identity == &source_identity
            )
        })
        .collect::<Vec<_>>();
    assert!(source_partitions.len() > 1, "{source_partitions:?}");
    assert!(source_partitions.len() <= 4, "{source_partitions:?}");

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut definitions = Vec::new();
    let mut declaration_counts = Vec::new();
    for module in output.modules.iter().filter(|module| {
        matches!(
            &module.key,
            CodegenUnitKey::SourceModule {
                source_identity: identity,
                ..
            } if identity == &source_identity
        )
    }) {
        let unit_definitions = module
            .ir
            .lines()
            .filter_map(|line| line.strip_prefix("define "))
            .filter_map(|line| line.split_once('@').map(|(_, suffix)| suffix))
            .filter_map(|suffix| suffix.split_once('(').map(|(symbol, _)| symbol.to_string()))
            .collect::<Vec<_>>();
        assert!(!unit_definitions.is_empty(), "{}", module.ir);
        definitions.extend(unit_definitions);
        declaration_counts.push(
            module
                .ir
                .lines()
                .filter(|line| line.starts_with("declare "))
                .count(),
        );
    }
    let unique = definitions.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), definitions.len(), "{definitions:?}");
    assert!(definitions.len() >= 8, "{definitions:?}");
    declaration_counts.sort_unstable();
    assert_eq!(declaration_counts, vec![0, 0, 0, 5]);
}

#[test]
fn partition_fingerprints_track_only_exact_declaration_dependencies() {
    fn source(helper0_ty: &str, helper1_ty: &str) -> String {
        format!(
            r#"
fn helper0() {helper0_ty} {{ 0 }}
fn helper1() {helper1_ty} {{ 1 }}
fn helper2() i32 {{ 2 }}
fn helper3() i32 {{ 3 }}
fn helper4() i32 {{ 4 }}
fn helper5() i32 {{ 5 }}
fn helper6() i32 {{ 6 }}

fn main() i64 {{
    helper0() as i64
}}
"#,
        )
    }

    fn fingerprints(
        output: &LlvmCodegenOutput,
    ) -> std::collections::BTreeMap<u32, nia_backend_ir::CodegenUnitFingerprint> {
        output
            .modules
            .iter()
            .filter_map(|module| match &module.key {
                CodegenUnitKey::SourceModule { ordinal, .. } => {
                    Some((*ordinal, module.fingerprint))
                }
                CodegenUnitKey::CompilerBuiltins => None,
            })
            .collect()
    }

    fn changed_partitions(
        baseline: &std::collections::BTreeMap<u32, nia_backend_ir::CodegenUnitFingerprint>,
        changed: &std::collections::BTreeMap<u32, nia_backend_ir::CodegenUnitFingerprint>,
    ) -> Vec<u32> {
        baseline
            .iter()
            .filter_map(|(&ordinal, fingerprint)| {
                (changed.get(&ordinal) != Some(fingerprint)).then_some(ordinal)
            })
            .collect()
    }

    let root = temp_dir("partition_fingerprints_track_only_exact_declaration_dependencies");
    let main = root.join("main.nia");
    std::fs::write(&main, source("i32", "i32")).expect("write baseline source");
    let baseline_codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(
        baseline_codegen.diagnostics.is_empty(),
        "{:?}",
        baseline_codegen.diagnostics
    );
    let baseline_output = emit_llvm_ir(
        &baseline_codegen.backend_lowering,
        &baseline_codegen.type_store,
    );
    assert!(
        baseline_output.diagnostics.is_empty(),
        "{:?}",
        baseline_output.diagnostics
    );
    let baseline = fingerprints(&baseline_output);

    std::fs::write(&main, source("i32", "i64")).expect("write unreferenced ABI change");
    let unreferenced_codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(
        unreferenced_codegen.diagnostics.is_empty(),
        "{:?}",
        unreferenced_codegen.diagnostics
    );
    let unreferenced_output = emit_llvm_ir(
        &unreferenced_codegen.backend_lowering,
        &unreferenced_codegen.type_store,
    );
    assert!(
        unreferenced_output.diagnostics.is_empty(),
        "{:?}",
        unreferenced_output.diagnostics
    );
    assert_eq!(
        changed_partitions(&baseline, &fingerprints(&unreferenced_output)).len(),
        1
    );

    std::fs::write(&main, source("i64", "i32")).expect("write referenced ABI change");
    let referenced_codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(
        referenced_codegen.diagnostics.is_empty(),
        "{:?}",
        referenced_codegen.diagnostics
    );
    let referenced_output = emit_llvm_ir(
        &referenced_codegen.backend_lowering,
        &referenced_codegen.type_store,
    );
    assert!(
        referenced_output.diagnostics.is_empty(),
        "{:?}",
        referenced_output.diagnostics
    );
    assert_eq!(
        changed_partitions(&baseline, &fingerprints(&referenced_output)).len(),
        2
    );
}

#[test]
fn array_len_only_large_module_emits_no_cross_partition_global_declarations() {
    let root = temp_dir("array_len_only_large_module_emits_no_cross_partition_global_declarations");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
static blob0: [4]u8 = [0u8; 4];
static blob1: [4]u8 = [1u8; 4];
static blob2: [4]u8 = [2u8; 4];
static blob3: [4]u8 = [3u8; 4];
static blob4: [4]u8 = [4u8; 4];
static blob5: [4]u8 = [5u8; 4];
static blob6: [4]u8 = [6u8; 4];
static blob7: [4]u8 = [7u8; 4];

fn main() usize {
    blob0.len() + blob1.len() + blob2.len() + blob3.len()
        + blob4.len() + blob5.len() + blob6.len() + blob7.len()
}
"#,
    )
    .expect("write array-len partition source");
    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let source_modules = output
        .modules
        .iter()
        .filter(|module| matches!(module.key, CodegenUnitKey::SourceModule { .. }))
        .collect::<Vec<_>>();
    assert!(source_modules.len() > 1);
    assert_eq!(
        source_modules
            .iter()
            .flat_map(|module| module.ir.lines())
            .filter(|line| line.starts_with('@') && line.contains(" = constant "))
            .count(),
        8
    );
    assert!(source_modules.iter().all(|module| {
        !module
            .ir
            .lines()
            .any(|line| line.starts_with('@') && line.contains(" = external constant "))
    }));
}

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
        let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
        assert!(
            output.diagnostics.is_empty(),
            "{} codegen diagnostics: {:?}",
            case.name,
            output.diagnostics
        );
        assert_eq!(
            output.modules.len(),
            codegen
                .backend_lowering
                .codegen_partitions
                .partitions()
                .len(),
            "{} should emit one LLVM module per source codegen partition",
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
            &codegen.backend_lowering,
            &codegen.type_store,
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
            codegen
                .backend_lowering
                .codegen_partitions
                .partitions()
                .len(),
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
