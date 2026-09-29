use super::*;

#[test]
fn rejected_syntax_publication_does_not_commit_source_text() {
    let sources = SourceDatabase::new();
    let path = SourcePath::new("main.nia");
    let file = sources.set_source(path.clone(), "fn main() { 0 }").unwrap();
    let loader = LoaderDatabase::new_for_test(test_request(path.as_str()).with_sources(sources));
    let original = loader.db.expect_get(SyntaxModuleQuery(file.version()));
    let next = SourceVersion {
        revision: file.revision.next().unwrap(),
        ..file.version()
    };
    // Occupy the target slot to exercise the publication failure before commit.
    let other = loader
        .sources
        .id_for_path(&SourcePath::new("other.nia"))
        .unwrap();
    loader
        .db
        .publish_shared(
            SyntaxModuleQuery(next),
            nia_syntax::parse("", Some(next)).unwrap(),
            &SourceTextQuery(other),
        )
        .unwrap();
    assert!(loader.set_source(path.as_str(), "fn main() { 1 }").is_err());
    let retained = loader.sources.source_for_path(&path).unwrap();
    assert_eq!(retained.version(), file.version());
    assert_eq!(retained.text, file.text);
    assert_eq!(
        *loader.db.expect_get(SyntaxModuleQuery(file.version())),
        *original
    );
    loader.db.retire(&SyntaxModuleQuery(next)).unwrap();
    let updated = loader.set_source(path.as_str(), "fn main() { 1 }").unwrap();
    assert_eq!(updated.version(), next);
    assert_eq!(
        *loader.db.expect_get(SyntaxModuleQuery(next)),
        nia_syntax::parse(&updated.text, Some(next)).unwrap()
    );
}

#[test]
fn loader_edits_reuse_syntax_and_match_clean_lowering_through_recovery() {
    let sources = SourceDatabase::new();
    let path = SourcePath::new("main.nia");
    let text = "fn main() { before(); if flag { value[index]; } after(); } fn last() {}";
    let mut file = sources.set_source(path.clone(), text).unwrap();
    let loader = LoaderDatabase::new_for_test(test_request(path.as_str()).with_sources(sources));
    let original = loader.db.expect_get(SyntaxModuleQuery(file.version()));
    let old_last = original
        .tree
        .green_root()
        .children()
        .iter()
        .rev()
        .find_map(|child| match child {
            nia_syntax::GreenElement::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    let mut previous = "value[index]";
    for (index, replacement) in [
        "longer[Box[N]]",
        "",
        "value[index]",
        "a + b",
        "value[index]",
    ]
    .into_iter()
    .enumerate()
    {
        let start = if previous.is_empty() {
            file.text.find("{ ;").unwrap() + 2
        } else {
            file.text.find(previous).unwrap()
        };
        let edit = nia_syntax::TextEdit::replace(
            nia_span::Span::new(start, start + previous.len()),
            replacement,
        );
        file = if index % 2 == 0 {
            loader.edit_source(file.version(), &edit).unwrap()
        } else {
            loader
                .set_source(path.as_str(), edit.apply(&file.text).unwrap())
                .unwrap()
        };
        let incremental = loader.db.expect_get(SyntaxModuleQuery(file.version()));
        let clean = nia_syntax::parse(&file.text, Some(file.version())).unwrap();
        assert_eq!(*incremental, clean);
        let new_last = incremental
            .tree
            .green_root()
            .children()
            .iter()
            .rev()
            .find_map(|child| match child {
                nia_syntax::GreenElement::Node(node) => Some(node),
                _ => None,
            })
            .unwrap();
        assert!(old_last.shares_structure(new_last));
        let parsed = loader.db.expect_get(parsed_module_query(&loader.db, &path));
        let lowered = nia_parser::lower_module(
            &clean,
            &loader.db.context().node_store,
            loader.db.context().symbols.clone(),
        );
        assert_eq!(
            parsed.semantic.item_tree,
            ModuleItemTree::from_owned_module(lowered.module)
        );
        assert_eq!(parsed.semantic.parse_errors, lowered.errors);
        assert_eq!(parsed.semantic.origins, lowered.origins);
        let trace = loader.query_trace().unwrap();
        assert_eq!(
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == "syntax_module")
                .count(),
            1
        );
        assert!(
            trace
                .dependencies
                .iter()
                .any(|edge| edge.from.name == "syntax_module" && edge.to.name == "source_text")
        );
        previous = replacement;
    }
    assert_eq!(original.tree.full_text(), text);
    loader.invalidate_source(path.as_str()).unwrap();
    let rebuilt = loader.db.expect_get(SyntaxModuleQuery(file.version()));
    assert_eq!(
        *rebuilt,
        nia_syntax::parse(&file.text, Some(file.version())).unwrap()
    );
}

#[test]
fn rejected_and_lazy_loader_edits_preserve_revision_ownership() {
    let sources = SourceDatabase::new();
    let path = SourcePath::new("main.nia");
    let file = sources
        .set_source(path.clone(), "// \u{4e2d}\nfn main() {}")
        .unwrap();
    let loader = LoaderDatabase::new_for_test(test_request(path.as_str()).with_sources(sources));
    let same = loader.set_source(path.as_str(), file.text.clone()).unwrap();
    assert!(
        loader
            .query_trace()
            .unwrap()
            .queries
            .iter()
            .all(|query| query.frame.name != "syntax_module")
    );
    let original = loader.db.expect_get(SyntaxModuleQuery(same.version()));
    let count = loader.query_trace().unwrap().queries.len();
    for (version, edit) in [
        (
            file.version(),
            nia_syntax::TextEdit::insert(0, "// stale\n"),
        ),
        (
            same.version(),
            nia_syntax::TextEdit::delete(nia_span::Span::new(4, 5)),
        ),
        (
            same.version(),
            nia_syntax::TextEdit::delete(nia_span::Span::new(0, 999)),
        ),
    ] {
        assert!(matches!(
            loader.edit_source(version, &edit),
            Err(nia_query::QueryError::InvalidInput { .. })
        ));
        assert_eq!(
            loader.sources.source_for_path(&path).unwrap().version(),
            same.version()
        );
        assert!(Arc::ptr_eq(
            &original,
            &loader.db.expect_get(SyntaxModuleQuery(same.version()))
        ));
        assert_eq!(loader.query_trace().unwrap().queries.len(), count);
    }
    for text in [
        "// \u{4e2d}\nfn main() {}",
        "// \u{4e01}\nfn main() {}",
        "// \u{6587}\nfn main() { }",
    ] {
        let updated = loader.set_source(path.as_str(), text).unwrap();
        assert_eq!(
            *loader.db.expect_get(SyntaxModuleQuery(updated.version())),
            nia_syntax::parse(text, Some(updated.version())).unwrap()
        );
    }
}

#[test]
fn compiler_loader_update_detaches_current_defs_from_old_source_revision() {
    let sources = SourceDatabase::new();
    sources
        .set_source(SourcePath::new("main.nia"), "fn main() i32 { 0 }")
        .expect("store source");
    let loader = LoaderDatabase::new_for_test(test_request("main.nia").with_sources(sources));
    let compiler = CompilerDatabase::new(CompileRequest::new(loader.clone()))
        .expect("create compiler database");

    let first = compiler.analyze_program().expect("initial analysis");
    assert!(!has_error_diagnostics(&first.diagnostics));
    let first_defs = Arc::clone(&first.modules[0].defs);
    assert!(
        first_defs
            .def_nodes
            .entries()
            .all(|(key, _)| key.revision == nia_source::SourceRevision::INITIAL)
    );

    let latest_source = loader
        .set_source("main.nia", "fn main() i32 { 1 }")
        .expect("replace source");
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
    let first_source = sources
        .set_source(path.clone(), "fn main() i32 { 1 }")
        .expect("store first source");
    let db = QueryDb::new(test_loader_context(
        path.clone(),
        ModuleMap::default(),
        sources.clone(),
    ))
    .expect("create loader query database");
    let declaration_key =
        SemanticFieldParent(first_source.id, SemanticFieldParentKind::Declaration);
    let signature_key =
        SemanticFieldParent(first_source.id, SemanticFieldParentKind::FunctionSignature);
    let first_declaration = db.expect_get(declaration_key);
    let first_signature = db.expect_get(signature_key);

    sources
        .set_source(path, "fn main() i32 { 2 }")
        .expect("store second source");
    db.invalidate(SourceTextQuery(first_source.id))
        .expect("invalidate source text");
    let latest_declaration = db.expect_get(declaration_key);
    let latest_signature = db.expect_get(signature_key);

    assert!(!Arc::ptr_eq(&first_declaration, &latest_declaration));
    assert!(!Arc::ptr_eq(&first_signature, &latest_signature));
    let trace = db.query_trace().expect("query trace");
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
