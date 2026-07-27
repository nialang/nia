use super::*;

#[test]
fn body_only_edits_reuse_item_signature_provider_summary() {
    let root = temp_dir("body_only_edits_reuse_item_signature_provider_summary");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let first_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));

    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);
    let first_identity = provider_cache_identity(&first_file);

    let edited_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 2 + 3 } }",
    );
    let edited_identity = provider_cache_identity(&edited_file);
    assert_ne!(first_identity.source_key, edited_identity.source_key);
    assert_eq!(
        first_identity.item_signature,
        edited_identity.item_signature
    );
    assert_eq!(first_identity.provider_key, edited_identity.provider_key);

    let edited = provider_summary_database(&main, &sources, cache.clone(), false);
    let edited_summary = edited.expect_get(provider_summary_query(&edited, &provider));
    assert_eq!(first_summary, edited_summary);
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        cache
            .load_dependency_manifest(
                edited_identity.source_key,
                edited_identity.namespace,
                &edited_identity.module,
                edited_identity.source,
            )
            .expect("load edited dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == first_identity.item_signature
    ));

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(edited_summary, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn signature_edits_publish_distinct_provider_summaries() {
    let root = temp_dir("signature_edits_publish_distinct_provider_summaries");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let first_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert!(first_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    let first_identity = provider_cache_identity(&first_file);

    let edited_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn rank(&self) i32 { 1 } }",
    );
    let edited_identity = provider_cache_identity(&edited_file);
    assert_ne!(
        first_identity.item_signature,
        edited_identity.item_signature
    );
    assert_ne!(first_identity.provider_key, edited_identity.provider_key);

    let edited = provider_summary_database(&main, &sources, cache.clone(), false);
    let edited_summary = edited.expect_get(provider_summary_query(&edited, &provider));
    assert!(!edited_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert!(edited_summary.defines_inherent_associated_item(&sym("Widget"), &sym("rank")));
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);
    assert!(
        cache
            .provider_summary_path(first_identity.provider_key)
            .is_file()
    );
    assert!(
        cache
            .provider_summary_path(edited_identity.provider_key)
            .is_file()
    );

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(edited_summary, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn provider_summary_verification_repairs_wrong_dependency_manifest() {
    let root = temp_dir("provider_summary_verification_repairs_wrong_dependency_manifest");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let provider_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let identity = provider_cache_identity(&provider_file);
    let wrong_item_signature = ItemSignatureFingerprint::from_parts([1, 2]);
    let wrong_provider_key = FrontendProviderSummaryCacheKey::new(
        identity.namespace,
        &identity.module,
        wrong_item_signature,
    );
    cache
        .publish_dependency_manifest(
            identity.source_key,
            identity.namespace,
            &identity.module,
            identity.source,
            wrong_item_signature,
        )
        .expect("publish wrong dependency manifest");
    cache
        .publish_provider_summary(
            wrong_provider_key,
            identity.namespace,
            &identity.module,
            wrong_item_signature,
            &nia_provider_summary::ProviderSummary::default(),
            &SymbolTable::new(),
        )
        .expect("publish provider summary for wrong dependency");

    let verifying = provider_summary_database(&main, &sources, cache.clone(), true);
    let verified = verifying.expect_get(provider_summary_query(&verifying, &provider));
    assert!(verified.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );
    assert!(matches!(
        cache
            .load_dependency_manifest(
                identity.source_key,
                identity.namespace,
                &identity.module,
                identity.source,
            )
            .expect("load repaired dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == identity.item_signature
    ));

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(verified, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}
