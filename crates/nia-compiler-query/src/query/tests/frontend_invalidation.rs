// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn extension_method_lookup_reuses_equal_facts_after_source_edits() {
    let body = "struct S { value: i32 } extend S { fun get(self) i32 { self.value } }";
    let mut fixture = LoadedProgramFixture::new("main.nia", body);
    let module = fixture.entry_id();
    let database = fixture.database();
    let db = &database.db;
    let facts = db.expect_get(ExtensionProviderModuleFactsQuery(module));
    let method = facts.methods.methods_named(&sym("get")).next().unwrap();
    let key = ExtensionMethodByIdQuery(method.def_id);
    let original = db.expect_get(key);
    assert_eq!(original.method.as_ref(), Some(method));

    fixture.update_module_source(
        module,
        &format!("// leading comment\n{body}"),
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    assert!(Arc::ptr_eq(&original, &db.expect_get(key)));

    fixture.update_module_source(
        module,
        &body.replace("fun get", "pub fun get"),
        SourceRevision(2),
    );
    database.update(CompileRequest::new(fixture.program()));
    let changed = db.expect_get(key);
    assert_ne!(
        original.method.as_ref().unwrap().visibility,
        changed.method.as_ref().unwrap().visibility
    );
}

#[test]
fn restricted_export_edits_invalidate_name_scopes_without_replacing_targets() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "pub module api; pub module source;");
    let entry = fixture.entry_id();
    let source = fixture.add_child(
        entry,
        "source",
        "source.nia",
        "pub struct Number {} pub fun answer() i32 { 42 }",
    );
    let api = fixture.add_child(
        entry,
        "api",
        "api.nia",
        "pub(root) using root::source::{Number, answer};",
    );
    let database = fixture.database();
    let deny_restricted = |_| false;
    for (revision, prefix, expected) in [
        (1, "pub      ", true),
        (2, "pub(root)", false),
        (3, "pub      ", true),
    ] {
        let before = database
            .db
            .expect_get(PublicSurfaceTypeQuery(api, sym("Number")));
        assert_eq!(before.as_ref().as_ref().unwrap().target_module, source);
        fixture.update_module_source(
            api,
            &format!("{prefix} using root::source::{{Number, answer}};"),
            SourceRevision(revision),
        );
        database.update(CompileRequest::new(fixture.program()));
        let lookup = QueryPublicSurfaceLookup::new(&database.db);
        assert_eq!(
            lookup
                .lookup_type(api, &sym("Number"), &deny_restricted)
                .into_visible()
                .is_some(),
            expected
        );
        assert_eq!(
            lookup
                .lookup_value(api, &sym("answer"), &deny_restricted)
                .into_visible()
                .is_some(),
            expected
        );
        let after = database
            .db
            .expect_get(PublicSurfaceTypeQuery(api, sym("Number")));
        assert_eq!(before.as_ref(), after.as_ref());
    }
}

#[test]
fn semantic_module_ids_include_semantic_only_modules_and_exclude_shallow_facades() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
pub module facade;

fun main() i32 {
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let semantic_id =
        fixture.add_shallow_child(entry_id, "semantic", "semantic.nia", "pub struct Needed {}");
    assert!(fixture.graph.mark_semantic_selected(semantic_id));
    let facade_id = fixture.add_shallow_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
pub fun expensive_or_invalid() i32 {
missing_symbol
}
"#,
    );
    let db = query_db(fixture.program());

    assert_eq!(
        resolve_stable_module_sequence(&db, &db.expect_get(ParseOkModuleIdsQuery))
            .expect("parse-ok module sequence")
            .as_slice(),
        &[entry_id, semantic_id, facade_id]
    );
    assert_eq!(
        resolve_stable_module_sequence(&db, &db.expect_get(SemanticModuleIdsQuery))
            .expect("semantic module sequence")
            .as_slice(),
        &[entry_id, semantic_id]
    );

    let body_worklist = db.expect_get(BodyActivationWorklistQuery);
    assert_eq!(body_worklist.modules.len(), 1);
    assert_eq!(
        body_worklist.modules.values().copied().next(),
        Some(entry_id)
    );
}

#[test]
fn loader_facts_map_modules_by_source_identity() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let package_id =
        fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fun value() i32 { 1 }");
    let db = query_db(fixture.program());

    assert_eq!(
        module_id_for_source_identity(&db, &SourcePath::new("pkg/root.nia").identity()),
        Some(package_id)
    );
}

#[test]
fn loaded_module_reorder_invalidates_list_without_field_changes() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let package_id =
        fixture.add_child(entry_id, "pkg", "pkg/root.nia", "pub fun value() i32 { 1 }");
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
fn removing_a_module_preserves_independent_facts_and_their_input_edges() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "pub module keep;");
    let entry = fixture.entry_id();
    let keep = fixture.add_child(entry, "keep", "keep.nia", "pub struct Before {}");
    let reduced = fixture.program();
    let removed = fixture.add_child(entry, "extra", "extra.nia", "pub struct Extra {}");
    let database = fixture.database();
    let before = database.db.expect_get(ModuleDefsQuery(keep));
    let extra = database.db.expect_get(ModuleDefsQuery(removed));
    database.update(CompileRequest::new(reduced));
    assert!(Arc::ptr_eq(
        &before,
        &database.db.expect_get(ModuleDefsQuery(keep))
    ));
    assert!(
        !database
            .query_trace()
            .unwrap()
            .queries
            .iter()
            .any(|query| query.frame.name == "module_defs"
                && query.frame.description.as_ref() == ModuleDefsQuery(removed).description())
    );
    fixture.update_module_source(keep, "pub struct After {}", SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));
    assert!(!Arc::ptr_eq(
        &extra,
        &database.db.expect_get(ModuleDefsQuery(removed))
    ));
    let after = database.db.expect_get(ModuleDefsQuery(keep));
    assert!(
        after
            .semantic
            .module_scope
            .types
            .get(&sym("Before"))
            .is_none()
    );
    assert!(
        after
            .semantic
            .module_scope
            .types
            .get(&sym("After"))
            .is_some()
    );
}

#[test]
fn additive_module_growth_preserves_global_executable_epoch() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let database = fixture.database();
    let first = database.db.expect_get(ExecutableFactEpochQuery);
    fixture.add_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fun value() i32 { 1 }",
    );
    database.update(CompileRequest::new(fixture.program()));
    let latest = database.db.expect_get(ExecutableFactEpochQuery);

    assert_eq!(first.as_ref(), latest.as_ref());
}

#[test]
fn stable_graph_entry_remaps_after_module_graph_owner_replacement() {
    let old_fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let new_fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let old_entry = old_fixture.entry_id();
    let new_entry = new_fixture.entry_id();
    assert_ne!(old_entry, new_entry);
    let database = old_fixture.database();
    let first = database.db.expect_get(ModuleGraphEntryQuery);
    let first_module_id = database
        .db
        .expect_get(CurrentModuleIdQuery(first.as_ref().clone()));
    let first_loaded = database.db.expect_get(LoadedModulesQuery);
    assert_eq!(*first_module_id, Some(old_entry));

    database.update(CompileRequest::new(new_fixture.program()));

    let latest = database.db.expect_get(ModuleGraphEntryQuery);
    let latest_module_id = database
        .db
        .expect_get(CurrentModuleIdQuery(latest.as_ref().clone()));
    let latest_loaded = database.db.expect_get(LoadedModulesQuery);
    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(first.as_ref(), latest.as_ref());
    assert_eq!(*latest_module_id, Some(new_entry));
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
fn stable_graph_relations_reuse_handles_across_graph_forks() {
    let base = LoadedProgramFixture::new(
        "main.nia",
        "pub module child; using self::child; fun main() i32 { 0 }",
    );
    let entry = base.entry_id();
    let mut old_fixture = LoadedProgramFixture {
        graph: base.graph.clone(),
        modules: base.modules.clone(),
        sources: base.sources.clone(),
    };
    let mut new_fixture = LoadedProgramFixture {
        graph: base.graph.clone(),
        modules: base.modules,
        sources: base.sources,
    };
    let child_name = sym("child");
    let package = sym("pkg");
    let old_child = old_fixture.add_child(
        entry,
        "child",
        "main/child.nia",
        "pub fun value() i32 { 1 }",
    );
    let new_child = new_fixture.add_child(
        entry,
        "child",
        "main/child.nia",
        "pub fun value() i32 { 1 }",
    );
    let old_root = old_fixture
        .graph
        .intern_package_root(&package, SourcePath::new("pkg/root.nia"))
        .expect("intern old package root");
    old_fixture.modules.push(loaded_module(
        old_root,
        "pkg/root.nia",
        "pub fun value() i32 { 1 }",
    ));
    let new_root = new_fixture
        .graph
        .intern_package_root(&package, SourcePath::new("pkg/root.nia"))
        .expect("intern new package root");
    new_fixture.modules.push(loaded_module(
        new_root,
        "pkg/root.nia",
        "pub fun value() i32 { 1 }",
    ));
    assert_eq!(old_child, new_child);
    assert_eq!(old_root, new_root);
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
    assert_eq!(first_child.as_ref(), latest_child.as_ref());
    assert_eq!(first_root.as_ref(), latest_root.as_ref());
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
    let source = "pub struct S { value: i32 } fun main() i32 { 0 }";
    let old_fixture = LoadedProgramFixture::new("main.nia", source);
    let old_program = old_fixture.program();

    let mut new_fixture = LoadedProgramFixture::new("bootstrap.nia", "");
    let new_module_id = new_fixture
        .graph
        .intern_package_root(&sym("replacement"), SourcePath::new("main.nia"))
        .expect("intern replacement package root");
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
    let mut fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let module_id = fixture.entry_id();
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));

    fixture.update_module_source(module_id, "fun main() i32 { true }", SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));
    let latest_source_version = database.db.expect_get(ModuleSourceVersionQuery(module_id));
    assert!(!Arc::ptr_eq(&first_source_version, &latest_source_version));
    assert_eq!(latest_source_version.revision, SourceRevision(1));

    let second = database.check_program();
    assert!(!second.diagnostics.is_empty());
    assert!(
        database
            .query_trace()
            .expect("query trace")
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
    let source = "fun main() i32 { 0 }";
    let mut fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let first = database.db.expect_get(ModuleSourceVersionQuery(module_id));
    let replacement = SourceVersion {
        id: SourceId::isolated(),
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
    let fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { 0 }");
    let loaded = fixture.program();
    let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let before_update = database.query_trace().expect("query trace");

    let invalidation =
        database.update(CompileRequest::new(loaded).with_timings(crate::TimingMode::Summary));
    assert!(
        invalidation.invalidated.is_empty(),
        "{:?}",
        invalidation.invalidated
    );

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let after_second_check = database.query_trace().expect("query trace");

    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_program");
    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_module_ids");
    assert_query_executions_unchanged(&before_update, &after_second_check, "checked_module");
}
