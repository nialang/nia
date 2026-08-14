// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn codegen_public_adapter_reuses_large_product_handles() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let cached = database.db.expect_get(CodegenProgramQuery);
    let owned = database.codegen_program();

    assert!(Arc::ptr_eq(
        &cached.monomorphization,
        &owned.monomorphization
    ));
    assert!(Arc::ptr_eq(
        &cached.backend_lowering,
        &owned.backend_lowering
    ));
}

#[test]
fn codegen_preparation_does_not_cross_backend_aggregate_barrier() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let preparation = database.codegen_preparation();

    assert!(preparation.diagnostics.is_empty());
    let trace = database.query_trace();
    assert_eq!(query_executions(&trace, "codegen_preparation"), 1);
    assert_eq!(query_executions(&trace, "backend_lowering"), 0);
    assert_eq!(query_executions(&trace, "backend_module_finalization"), 0);
}

#[test]
fn scoped_backend_schedule_exposes_each_module_before_aggregate_finish() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module helper; using entry::helper; fn main() i32 { helper::value() }",
    );
    let entry = fixture.entry_id();
    let helper = fixture.add_child(entry, "helper", "helper.nia", "pub fn value() i32 { 1 }");
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));
    let preparation = database.codegen_preparation();
    assert!(preparation.diagnostics.is_empty());

    let lowering = database
        .with_backend_finalization_schedule(|schedule| {
            let mut schedule = schedule.expect("healthy preparation must produce a schedule");
            let store = schedule.module_store();
            assert!(store.get(entry).is_none());
            assert!(store.get(helper).is_none());

            let ready = schedule
                .wait_next()
                .expect("backend finalization query")
                .expect("first backend module");
            assert!(store.get(ready.module_id()).is_some());
            let other = if ready.module_id() == entry {
                helper
            } else {
                entry
            };
            assert!(store.get(other).is_none());
            schedule.finish()
        })
        .expect("backend finalization schedule")
        .expect("backend finalization queries");

    assert_eq!(lowering.program.modules.len(), 2);
    assert!(
        lowering
            .program
            .modules
            .iter()
            .any(|module| module.id == entry)
    );
    assert!(
        lowering
            .program
            .modules
            .iter()
            .any(|module| module.id == helper)
    );
    let trace = database.query_trace();
    assert_eq!(query_executions(&trace, "backend_lowering"), 0);
    assert_eq!(query_executions(&trace, "backend_module_finalization"), 2);
}

#[test]
fn backend_definition_manifest_precedes_finalization_at_every_optimization_level() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module geom;
using entry::geom;

fn main() i32 {
let mut point = geom::Point { x: 40, y: 2 };
point.x + point.y
}
"#,
    );
    let entry = fixture.entry_id();
    let geom = fixture.add_child(
        entry,
        "geom",
        "geom.nia",
        r#"
pub struct Point {
x: i32,
y: i32,
}
"#,
    );

    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::O3,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let mut program = fixture.program();
        program.runtime = RuntimeModel::FreestandingExecutable;
        let database = CompilerDatabase::new(CompileRequest::new(program).with_optimization(level));
        let preparation = database.codegen_preparation();
        assert!(
            preparation.diagnostics.is_empty(),
            "{level:?}: {:?}",
            preparation.diagnostics
        );
        let defs = database.db.expect_get(FullModuleDefsQuery(geom));
        let point = defs
            .semantic
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("Point") && def.kind == nia_defs::DefKind::Struct).then_some(
                    GlobalDefId {
                        module_id: geom,
                        def_id,
                    },
                )
            })
            .expect("Point definition");

        database
            .with_backend_finalization_schedule(|schedule| {
                let mut schedule = schedule.expect("healthy preparation must produce a schedule");
                assert_eq!(schedule.owner_directory().item_owner(point), Some(geom));
                assert!(schedule.module_store().get(geom).is_none());
                while schedule
                    .wait_next()
                    .expect("backend finalization query")
                    .is_some()
                {}
                let lowering = schedule.finish().expect("backend finalization queries");
                let geom_module = lowering
                    .program
                    .modules
                    .iter()
                    .find(|module| module.id == geom)
                    .expect("finalized geom module");
                assert!(geom_module.structs.iter().any(|item| item.def_id == point));
            })
            .expect("backend finalization schedule");
    }
}
