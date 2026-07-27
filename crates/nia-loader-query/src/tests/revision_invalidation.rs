use super::*;

#[test]
fn compiler_loader_update_detaches_current_defs_from_old_source_revision() {
    let sources = SourceDatabase::new();
    sources.set_source(SourcePath::new("main.nia"), "fn main() i32 { 0 }");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_sources(sources));
    let compiler = CompilerDatabase::new(CompileRequest::new(loader.clone()));

    let first = compiler.analyze_program().expect("initial analysis");
    assert!(!has_error_diagnostics(&first.diagnostics));
    let first_defs = Arc::clone(&first.modules[0].defs);
    assert!(
        first_defs
            .def_nodes
            .entries()
            .all(|(key, _)| key.revision == nia_source::SourceRevision::INITIAL)
    );

    let latest_source = loader.set_source("main.nia", "fn main() i32 { 1 }");
    compiler
        .update(CompileRequest::new(loader.clone()))
        .expect("compiler update");
    let latest = compiler.analyze_program().expect("updated analysis");

    assert!(!has_error_diagnostics(&latest.diagnostics));
    let latest_defs = &latest.modules[0].defs;
    assert!(!Arc::ptr_eq(&first_defs, latest_defs));
    assert!(
        latest_defs
            .def_nodes
            .entries()
            .all(|(key, _)| key.source_version() == latest_source.version())
    );
    assert!(
        first_defs
            .def_nodes
            .entries()
            .all(|(key, _)| key.revision == nia_source::SourceRevision::INITIAL)
    );
}

#[test]
fn body_only_source_change_refreshes_revision_bearing_field_dependents() {
    let sources = SourceDatabase::new();
    let path = SourcePath::new("main.nia");
    let first_source = sources.set_source(path.clone(), "fn main() i32 { 1 }");
    let db = QueryDb::new(test_loader_context(
        path.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));
    let declaration_key =
        SemanticFieldParent(first_source.id, SemanticFieldParentKind::Declaration);
    let signature_key =
        SemanticFieldParent(first_source.id, SemanticFieldParentKind::FunctionSignature);
    let first_declaration = db.expect_get(declaration_key);
    let first_signature = db.expect_get(signature_key);

    sources.set_source(path, "fn main() i32 { 2 }");
    db.invalidate(SourceTextQuery(first_source.id));
    let latest_declaration = db.expect_get(declaration_key);
    let latest_signature = db.expect_get(signature_key);

    assert!(!Arc::ptr_eq(&first_declaration, &latest_declaration));
    assert!(!Arc::ptr_eq(&first_signature, &latest_signature));
    let trace = db.query_trace();
    let declaration_fact = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "loader_module_item_tree_fact")
        .expect("declaration field fact");
    assert_eq!(declaration_fact.stats.executions, 2);
    assert_eq!(declaration_fact.stats.green_validations, 0);
    let signature_fact = trace
        .queries
        .iter()
        .find(|query| {
            query.frame.name == "loader_active_module_item_tree_fact"
                && query.frame.description.contains("Signature(Functions)")
        })
        .expect("function signature field fact");
    assert_eq!(signature_fact.stats.executions, 2);
    assert_eq!(signature_fact.stats.green_validations, 0);
    let parents = trace
        .queries
        .iter()
        .filter(|query| query.frame.name == "semantic_field_parent")
        .collect::<Vec<_>>();
    assert_eq!(parents.len(), 2);
    assert!(parents.iter().all(|query| query.stats.executions == 2));
    assert!(
        parents
            .iter()
            .all(|query| query.stats.green_validations == 0)
    );
}
