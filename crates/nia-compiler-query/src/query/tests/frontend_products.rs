// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn public_surface_snapshots_are_query_derived_facts() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(PublicSurfacesQuery);
    let _ = db.expect_get(ModulePublicSurfaceQuery(module_id));
    let _ = db.expect_get(ModuleUsingScopeQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "public_surfaces"
            && dependency.to.name == "public_surface_module_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        matches!(
            dependency.from.name,
            "public_surfaces" | "public_using_scopes" | "public_surface_module_facts"
        ) && dependency.to.name == "module_defs"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "public_surfaces" && dependency.to.name == "module_graph"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "public_using_scopes" && dependency.to.name == "public_surfaces"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "public_using_scopes"
            && dependency.to.name == "public_surface_module_facts"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_using_scope" && dependency.to.name == "public_using_scopes"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_public_surface" && dependency.to.name == "public_surfaces"
    }));
}

#[test]
fn item_tree_queries_reuse_single_layer_product_handles() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let module_input: Arc<ModuleItemTree> = db.expect_get(ModuleItemTreeInputQuery(module_id));
    let active_input: Arc<ActiveModuleItemTree> =
        db.expect_get(ActiveModuleItemTreeInputQuery(module_id));
    let full_module: Arc<ModuleItemTree> = db.expect_get(FullModuleItemTreeQuery(module_id));
    let full_active: Arc<ActiveModuleItemTree> =
        db.expect_get(FullActiveModuleItemTreeQuery(module_id));

    let module_input_batch = db
        .get_many([ModuleItemTreeInputQuery(module_id)])
        .expect("module input batch should succeed");
    let active_input_batch = db
        .get_many([ActiveModuleItemTreeInputQuery(module_id)])
        .expect("active input batch should succeed");
    let full_module_batch = db
        .get_many([FullModuleItemTreeQuery(module_id)])
        .expect("full module batch should succeed");
    let full_active_batch = db
        .get_many([FullActiveModuleItemTreeQuery(module_id)])
        .expect("full active batch should succeed");

    assert!(Arc::ptr_eq(&module_input, &module_input_batch[0]));
    assert!(Arc::ptr_eq(&active_input, &active_input_batch[0]));
    assert!(Arc::ptr_eq(&full_module, &full_module_batch[0]));
    assert!(Arc::ptr_eq(&full_active, &full_active_batch[0]));
}
