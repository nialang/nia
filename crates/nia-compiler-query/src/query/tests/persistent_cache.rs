// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn persistent_check_certificate_reuses_diagnostics_and_verifies_fresh() {
    static CACHE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let cache_id = CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "nia-clean-check-certificate-{}-{cache_id}",
        std::process::id()
    ));
    let compile = |source: &str, verify: bool| {
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let db = query_db_with_frontend_cache(
            fixture.program(),
            HashMap::from([(
                module_id,
                (crate::source_content_fingerprint(source), source.len()),
            )]),
            root.clone(),
            verify,
        );
        let inputs = Arc::clone(&db.context().inputs);
        let database = super::super::CompilerDatabase { db, inputs };
        let report = database
            .entry_check_program()
            .expect("test entry check report");
        (report, database.query_trace())
    };

    let source = "fn main() i32 { 1 }";
    let (cold, cold_trace) = compile(source, false);
    assert!(cold.diagnostics.is_empty(), "{:?}", cold.diagnostics);
    assert_eq!(query_executions(&cold_trace, "entry_checked_program"), 1);
    assert!(query_executions(&cold_trace, "executable_checked_module_facts") > 0);

    let (warm, warm_trace) = compile(source, false);
    assert_eq!(warm.optimization, cold.optimization);
    assert_eq!(warm.diagnostics, cold.diagnostics);
    assert_eq!(warm.checked_body_count(), cold.checked_body_count());
    assert_eq!(warm.reachable_body_count(), cold.reachable_body_count());
    assert_eq!(
        warm.graph
            .stable_key(warm.graph.entry())
            .expect("warm entry stable key"),
        cold.graph
            .stable_key(cold.graph.entry())
            .expect("cold entry stable key")
    );
    assert!(!warm.graph.ptr_eq(&cold.graph));
    assert_eq!(query_executions(&warm_trace, "entry_checked_program"), 0);
    assert_eq!(
        query_executions(&warm_trace, "executable_checked_module_facts"),
        0
    );
    assert_eq!(query_executions(&warm_trace, "body_check"), 0);

    let (verified, verified_trace) = compile(source, true);
    assert_eq!(verified.optimization, cold.optimization);
    assert_eq!(verified.diagnostics, cold.diagnostics);
    assert_eq!(verified.checked_body_count(), cold.checked_body_count());
    assert_eq!(
        query_executions(&verified_trace, "entry_checked_program"),
        1
    );

    let invalid_source = "fn main() i32 { true }";
    let (edited, edited_trace) = compile(invalid_source, false);
    assert!(!edited.diagnostics.is_empty());
    assert_eq!(query_executions(&edited_trace, "entry_checked_program"), 1);
    let (warm_invalid, warm_invalid_trace) = compile(invalid_source, false);
    assert_eq!(warm_invalid.diagnostics, edited.diagnostics);
    assert_eq!(
        query_executions(&warm_invalid_trace, "entry_checked_program"),
        0
    );

    let invalid_fixture = LoadedProgramFixture::new("main.nia", invalid_source);
    let invalid_module = invalid_fixture.entry_id();
    let invalid_db = query_db_with_frontend_cache(
        invalid_fixture.program(),
        HashMap::from([(
            invalid_module,
            (
                crate::source_content_fingerprint(invalid_source),
                invalid_source.len(),
            ),
        )]),
        root.clone(),
        false,
    );
    let invalid_inputs = Arc::clone(&invalid_db.context().inputs);
    let invalid_database = super::super::CompilerDatabase {
        db: invalid_db,
        inputs: invalid_inputs,
    };
    let invalid_context = invalid_database
        .check_certificate_context(FrontendCheckScope::Entry)
        .expect("certificate context query")
        .expect("invalid-source certificate context");
    invalid_database
        .db
        .context()
        .signature_cache
        .as_ref()
        .expect("signature cache")
        .publish_check_certificate(
            invalid_context.identity(),
            crate::signature_cache::CachedCheckCertificate {
                checked_body_count: 1,
                reachable_body_count: 1,
                diagnostics: Vec::new(),
            },
            true,
        )
        .expect("inject semantically wrong clean certificate");
    let (trusted_invalid, trusted_invalid_trace) = compile(invalid_source, false);
    assert!(trusted_invalid.diagnostics.is_empty());
    assert_eq!(
        query_executions(&trusted_invalid_trace, "entry_checked_program"),
        0
    );
    let (verified_invalid, verified_invalid_trace) = compile(invalid_source, true);
    assert_eq!(verified_invalid.diagnostics, edited.diagnostics);
    assert_eq!(
        query_executions(&verified_invalid_trace, "entry_checked_program"),
        1
    );
    let (retired_invalid, retired_invalid_trace) = compile(invalid_source, false);
    assert_eq!(retired_invalid.diagnostics, verified_invalid.diagnostics);
    assert_eq!(
        query_executions(&retired_invalid_trace, "entry_checked_program"),
        0
    );
    let (_, warm_after_verification) = compile(source, false);
    assert_eq!(
        query_executions(&warm_after_verification, "entry_checked_program"),
        0
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn persistent_executable_value_ref_edges_skip_resolution_and_verify_replacement() {
    static CACHE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let cache_id = CACHE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "nia-executable-value-ref-edges-{}-{cache_id}",
        std::process::id()
    ));
    let source = "fn helper() i32 { 1 } fn main() i32 { helper() }";
    let source_fingerprint = crate::source_content_fingerprint(source);
    let compile = |verify| {
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let db = query_db_with_frontend_cache(
            fixture.program(),
            HashMap::from([(module_id, (source_fingerprint, source.len()))]),
            root.clone(),
            verify,
        );
        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let owner = GlobalDefId {
            module_id,
            def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
        };
        let helper = GlobalDefId {
            module_id,
            def_id: defs
                .semantic
                .module_scope
                .values
                .get(&sym("helper"))
                .unwrap(),
        };
        let edges = db.expect_get(ExecutableValueRefEdgesQuery(owner));
        (owner, edges.functions.contains(&helper), db.query_trace())
    };

    let (cold_owner, cold_contains_helper, cold) = compile(false);
    assert!(cold_contains_helper);
    assert!(trace_has_dependency(
        &cold,
        "executable_value_ref_edges",
        "executable_value_ref_item"
    ));

    let (_, warm_contains_helper, warm) = compile(false);
    assert!(warm_contains_helper);
    assert!(trace_has_dependency(
        &warm,
        "executable_value_ref_edges",
        "frontend_program_sources"
    ));
    assert!(!trace_has_dependency(
        &warm,
        "executable_value_ref_edges",
        "executable_value_ref_item"
    ));
    assert!(!trace_has_dependency(
        &warm,
        "executable_value_ref_edges",
        "full_active_module_item_tree"
    ));

    let module = StableModuleKey::from_source_identity(SourceIdentity::new("main.nia"));
    let program_sources =
        crate::frontend_program_source_fingerprint([(&module, source_fingerprint, source.len())]);
    let namespace = crate::FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
    let key = crate::FrontendExecutableValueRefEdgesCacheKey::new(
        namespace,
        &module,
        cold_owner.def_id,
        program_sources,
    );
    let cache = crate::signature_cache::PersistentSignatureCache::new(root.clone());
    cache.remove_executable_value_ref_edges(key);
    cache
        .publish_executable_value_ref_edges(
            crate::signature_cache::ExecutableValueRefEdgesIdentity {
                key,
                namespace,
                module: &module,
                owner: cold_owner.def_id,
                program_sources,
            },
            &crate::signature_cache::CachedExecutableValueRefEdges::default(),
            &HashMap::from([(cold_owner.module_id, "main.nia".to_string())]),
            false,
        )
        .expect("publish semantically wrong value-ref edges");

    let (_, verified_contains_helper, verified) = compile(true);
    assert!(verified_contains_helper);
    assert!(trace_has_dependency(
        &verified,
        "executable_value_ref_edges",
        "executable_value_ref_item"
    ));

    let (_, replaced_contains_helper, replaced) = compile(false);
    assert!(replaced_contains_helper);
    assert!(!trace_has_dependency(
        &replaced,
        "executable_value_ref_edges",
        "executable_value_ref_item"
    ));
    let _ = std::fs::remove_dir_all(root);
}
