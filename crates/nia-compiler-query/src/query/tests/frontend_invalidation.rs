// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn semantic_module_ids_exclude_shallow_facade_modules() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
pub module facade;

fn main() i32 {
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let facade_id = fixture.add_shallow_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
pub fn expensive_or_invalid() i32 {
missing_symbol
}
"#,
    );
    let db = query_db(fixture.program());

    assert_eq!(
        resolve_stable_module_sequence(&db, &db.expect_get(ParseOkModuleIdsQuery))
            .expect("parse-ok module sequence")
            .as_slice(),
        &[entry_id, facade_id]
    );
    assert_eq!(
        resolve_stable_module_sequence(&db, &db.expect_get(SemanticModuleIdsQuery))
            .expect("semantic module sequence")
            .as_slice(),
        &[entry_id]
    );

    assert_eq!(db.expect_get(CheckedModuleIdsQuery).as_slice(), &[entry_id]);
}

#[test]
fn loader_facts_map_modules_by_source_identity() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let package_id = fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fn value() i32 { 1 }");
    let db = query_db(fixture.program());

    assert_eq!(
        module_id_for_source_identity(&db, &SourcePath::new("pkg/root.nia").identity()),
        Some(package_id)
    );
}

#[test]
fn loaded_module_reorder_invalidates_list_without_field_changes() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let package_id = fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fn value() i32 { 1 }");
    let database = fixture.database();
    let first = database.db.expect_get(LoadedModulesQuery);
    let mut reordered = fixture.program();
    reordered.modules.reverse();
    database.update(CompileRequest::new(reordered));
    let latest = database.db.expect_get(LoadedModulesQuery);
    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(
        resolve_stable_module_sequence(&database.db, &latest).expect("reordered module sequence"),
        vec![package_id, entry_id]
    );
}

#[test]
fn additive_module_growth_refreshes_query_derived_executable_epoch() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let database = fixture.database();
    let first = database.db.expect_get(ExecutableFactEpochQuery);
    fixture.add_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fn value() i32 { 1 }",
    );
    database.update(CompileRequest::new(fixture.program()));
    let latest = database.db.expect_get(ExecutableFactEpochQuery);

    assert_ne!(first.as_ref(), latest.as_ref());
    assert_eq!(first.modules.len() + 1, latest.modules.len());
    assert_eq!(first.modules[0], latest.modules[0]);
}

#[test]
fn stable_graph_entry_remaps_after_module_graph_owner_replacement() {
    let old_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let new_fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let old_entry = old_fixture.entry_id();
    let new_entry = new_fixture.entry_id();
    assert_ne!(old_entry, new_entry);
    let database = old_fixture.database();
    let first = database.db.expect_get(ModuleGraphEntryQuery);
    let first_loaded = database.db.expect_get(LoadedModulesQuery);

    database.update(CompileRequest::new(new_fixture.program()));

    let latest = database.db.expect_get(ModuleGraphEntryQuery);
    let latest_loaded = database.db.expect_get(LoadedModulesQuery);
    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(first.as_ref(), latest.as_ref());
    assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
    assert_eq!(
        resolve_stable_module_sequence(&database.db, &latest_loaded)
            .expect("remapped module sequence"),
        vec![new_entry]
    );
    assert_eq!(
        QueryModuleGraphLookup::new(&database.db)
            .expect("module graph lookup should load")
            .entry_module(),
        new_entry
    );
}

#[test]
fn stable_graph_relations_remap_fork_local_module_handles() {
    let base = LoadedProgramFixture::new(
        "main.nia",
        "pub module child; using self::child; fn main() i32 { 0 }",
    );
    let entry = base.entry_id();
    let mut old_fixture = LoadedProgramFixture {
        graph: base.graph.clone(),
        modules: base.modules.clone(),
    };
    let mut new_fixture = LoadedProgramFixture {
        graph: base.graph.clone(),
        modules: base.modules,
    };
    let child_name = sym("child");
    let package = sym("pkg");
    let old_child =
        old_fixture.add_child(entry, "child", "main/child.nia", "pub fn value() i32 { 1 }");
    let new_child =
        new_fixture.add_child(entry, "child", "main/child.nia", "pub fn value() i32 { 1 }");
    let old_root = old_fixture
        .graph
        .intern_package_root(&package, SourcePath::new("pkg/root.nia"));
    old_fixture.modules.push(loaded_module(
        old_root,
        "pkg/root.nia",
        "pub fn root() i32 { 1 }",
    ));
    let new_root = new_fixture
        .graph
        .intern_package_root(&package, SourcePath::new("pkg/root.nia"));
    new_fixture.modules.push(loaded_module(
        new_root,
        "pkg/root.nia",
        "pub fn root() i32 { 1 }",
    ));
    assert_ne!(old_child, new_child);
    assert_ne!(old_root, new_root);
    let database = old_fixture.database();
    let first_child = database
        .db
        .expect_get(ModuleGraphChildQuery(entry, child_name));
    let first_root = database.db.expect_get(ModulePackageRootQuery(package));
    let first_public = database
        .db
        .expect_get(PublicSurfaceModuleQuery(entry, child_name));
    let first_using = database
        .db
        .expect_get(UsingScopeModuleQuery(entry, child_name));

    database.update(CompileRequest::new(new_fixture.program()));

    let latest_child = database
        .db
        .expect_get(ModuleGraphChildQuery(entry, child_name));
    let latest_root = database.db.expect_get(ModulePackageRootQuery(package));
    let latest_public = database
        .db
        .expect_get(PublicSurfaceModuleQuery(entry, child_name));
    let latest_using = database
        .db
        .expect_get(UsingScopeModuleQuery(entry, child_name));
    assert!(!Arc::ptr_eq(&first_child, &latest_child));
    assert!(!Arc::ptr_eq(&first_root, &latest_root));
    assert_eq!(first_child.as_ref(), latest_child.as_ref());
    assert_eq!(first_root.as_ref(), latest_root.as_ref());
    assert!(!Arc::ptr_eq(&first_public, &latest_public));
    assert!(!Arc::ptr_eq(&first_using, &latest_using));
    assert_eq!(first_public.as_ref(), latest_public.as_ref());
    assert_eq!(first_using.as_ref(), latest_using.as_ref());
    let lookup =
        QueryModuleGraphLookup::new(&database.db).expect("module graph lookup should load");
    assert_eq!(
        lookup.child_declaration(entry, &child_name),
        Some((new_child, nia_ids::Visibility::Public))
    );
    assert_eq!(lookup.package_root_module(&package), Some(new_root));
    assert_eq!(
        QueryPublicSurfaceLookup::new(&database.db).public_module(entry, &child_name),
        Some(new_child)
    );
    assert_eq!(
        QueryUsingScopeLookup::new(&database.db, entry).using_module(&child_name),
        Some(new_child)
    );
}

#[test]
fn stable_source_identity_with_new_module_id_invalidates_old_key_and_recomputes_new_key() {
    let source = "pub struct S { value: i32 } fn main() i32 { 0 }";
    let old_fixture = LoadedProgramFixture::new("main.nia", source);
    let old_program = old_fixture.program();

    let mut new_fixture = LoadedProgramFixture::new("bootstrap.nia", "");
    let new_module_id = new_fixture
        .graph
        .intern_package_root(&sym("replacement"), SourcePath::new("main.nia"));
    new_fixture.graph.mark_process_used_paths(new_module_id);
    new_fixture.modules = vec![loaded_module(new_module_id, "main.nia", source)];
    let new_program = new_fixture.program();

    let database = CompilerDatabase::new(CompileRequest::new(old_program));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_loaded = database.db.expect_get(LoadedModulesQuery);

    database.update(CompileRequest::new(new_program));
    let latest_loaded = database.db.expect_get(LoadedModulesQuery);
    assert!(Arc::ptr_eq(&first_loaded, &latest_loaded));
    assert_eq!(
        resolve_stable_module_sequence(&database.db, &latest_loaded)
            .expect("updated module sequence"),
        vec![new_module_id]
    );

    let second = database.analyze_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    assert_eq!(second.modules[0].id, new_module_id);
}

#[test]
fn tracked_loader_update_refreshes_changed_module_field_inputs() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let module_id = fixture.entry_id();
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));

    fixture.update_module_source(module_id, "fn main() i32 { true }", SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));
    let latest_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));
    assert!(!Arc::ptr_eq(&first_source_version, &latest_source_version));
    assert_eq!(latest_source_version.revision, SourceRevision(1));

    let second = database.check_program();
    assert!(!second.diagnostics.is_empty());
    assert!(
        database
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "semantic_module_ids"
                    && dependency.to.name == "parse_ok_module_ids"
            })
    );
}

#[test]
fn source_handle_replacement_cannot_reuse_old_source_version() {
    let source = "fn main() i32 { 0 }";
    let mut fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let first = database.db.expect_get(ModuleSourceVersionQuery(module_id));
    let replacement = SourceVersion {
        id: SourceId(first.id.0 + 1),
        revision: first.revision,
    };
    fixture.modules[0] =
        loaded_module_with_source_version(module_id, "main.nia", source, replacement);

    database.update(CompileRequest::new(fixture.program()));
    let latest = database.db.expect_get(ModuleSourceVersionQuery(module_id));

    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(*latest, replacement);
}

#[test]
fn timing_mode_update_does_not_invalidate_semantic_queries() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let loaded = fixture.program();
    let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let before_update = database.query_trace();

    let invalidation =
        database.update(CompileRequest::new(loaded).with_timings(crate::TimingMode::Summary));
    assert!(
        invalidation.invalidated.is_empty(),
        "{:?}",
        invalidation.invalidated
    );

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let after_second_check = database.query_trace();

    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_program");
    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_module_ids");
    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_module");
}
