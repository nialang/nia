// SPDX-License-Identifier: GPL-3.0-or-later
mod common;

mod basic;
mod calls_and_slices;
mod control_flow;
mod cross_module;
mod function_ir;
mod imports_and_aggregates;
mod layouts_and_literals;
mod low_level_and_const;
mod operators;
mod smoke;
mod structural_extensions;
mod traits;
mod values_and_assignments;
mod void_and_empty;

#[test]
fn readiness_coordinator_retries_units_only_after_exact_owner_publication() {
    let root = common::temp_dir("readiness_coordinator_retries_exact_owner");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module geom;
using entry::geom;

fn main() i32 {
    let mut point: geom::Point = { x: 40, y: 2 };
    point.x + point.y
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("geom.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
    )
    .expect("write geom source");
    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let main_id = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main backend module")
        .id;
    let geom_id = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("geom.nia"))
        .expect("geom backend module")
        .id;
    let mut coordinator = crate::readiness::CodegenReadinessCoordinator::new(
        codegen.backend_lowering.program.module_store(),
        std::sync::Arc::clone(&codegen.type_store),
        std::sync::Arc::clone(&codegen.backend_lowering.owner_directory),
    );

    assert!(coordinator.publish(main_id).is_empty());
    let ready = coordinator.publish(geom_id);
    assert_eq!(ready.len(), 1);
    let crate::readiness::CodegenPartitionPreparation::Ready(prepared) = &ready[0] else {
        panic!("healthy pending unit became invalid")
    };
    assert!(matches!(
        prepared.partition.id,
        nia_backend_ir::CodegenUnitId::SourceModule { module_id, .. } if module_id == main_id
    ));
    assert_eq!(
        prepared.declarations.dependencies.modules(),
        &[main_id, geom_id]
    );
    let _ = coordinator.finish();
}

#[test]
fn closure_codegen_reports_the_materialization_boundary() {
    let root = common::temp_dir("closure_codegen_materialization_boundary");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(base: i32) i32 {
    let callback = [base](value: i32) i32 { base + value };
    callback(1)
}
"#,
    )
    .expect("write test source");

    let codegen = common::codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = common::emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("generated closure entry reached LLVM before backend materialization")
        }),
        "expected the closure materialization boundary diagnostic, got {:?}",
        output.diagnostics
    );
}
