use super::*;

#[test]
fn persistent_provider_summary_hit_skips_parse_and_recovers_from_corruption() {
    let root = temp_dir("persistent_provider_summary_hit_skips_parse_and_recovers_from_corruption");
    let cache_root = root.join("cache");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let provider_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        cache_root,
    ));
    let identity = provider_cache_identity(&provider_file);

    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert!(first_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);

    let second = provider_summary_database(&main, &sources, cache.clone(), false);
    let second_summary = second.expect_get(provider_summary_query(&second, &provider));
    assert_eq!(first_summary, second_summary);
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);

    let path = cache.provider_summary_path(identity.provider_key);
    fs::write(&path, b"corrupt frontend cache entry").expect("corrupt provider summary cache");
    let third = provider_summary_database(&main, &sources, cache.clone(), false);
    let third_summary = third.expect_get(provider_summary_query(&third, &provider));
    assert_eq!(first_summary, third_summary);
    assert_eq!(query_executions(&third.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        {
            let loaded_symbols = SymbolTable::new();
            cache
                .load_provider_summary(
                    identity.provider_key,
                    identity.namespace,
                    &identity.module,
                    identity.item_signature,
                    &loaded_symbols,
                )
                .expect("reload repaired provider summary")
        },
        crate::frontend_cache::ProviderSummaryCacheLookup::Hit(_)
    ));

    let manifest_path = cache.dependency_manifest_path(identity.source_key);
    fs::write(&manifest_path, b"corrupt frontend manifest").expect("corrupt dependency manifest");
    let fourth = provider_summary_database(&main, &sources, cache.clone(), false);
    let fourth_summary = fourth.expect_get(provider_summary_query(&fourth, &provider));
    assert_eq!(first_summary, fourth_summary);
    assert_eq!(query_executions(&fourth.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        cache
            .load_dependency_manifest(
                identity.source_key,
                identity.namespace,
                &identity.module,
                identity.source,
            )
            .expect("reload repaired dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == identity.item_signature
    ));

    let fifth = provider_summary_database(&main, &sources, cache, false);
    let fifth_summary = fifth.expect_get(provider_summary_query(&fifth, &provider));
    assert_eq!(first_summary, fifth_summary);
    assert_eq!(query_executions(&fifth.query_trace(), "parsed_module"), 0);
}

#[test]
fn provider_summary_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("provider_summary_verification_replaces_semantically_wrong_valid_entry");
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
    cache
        .publish_dependency_manifest(
            identity.source_key,
            identity.namespace,
            &identity.module,
            identity.source,
            identity.item_signature,
        )
        .expect("publish dependency manifest");
    cache
        .publish_provider_summary(
            identity.provider_key,
            identity.namespace,
            &identity.module,
            identity.item_signature,
            &nia_provider_summary::ProviderSummary::default(),
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid provider summary");

    let verifying = provider_summary_database(&main, &sources, cache.clone(), true);
    let verified = verifying.expect_get(provider_summary_query(&verifying, &provider));
    assert!(verified.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(verified, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}
