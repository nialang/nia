use super::*;

#[test]
fn module_dependencies_cache_keys_include_effective_module_map() {
    let root = temp_dir("module_dependencies_cache_keys_include_effective_module_map");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "using dep::Thing; fn main() () {}");
    let mut mapped = ModuleMap::new();
    mapped.insert("dep", SourcePath::new("deps/root.nia"));
    let unmapped = ModuleMap::new();
    let mapped_identity = module_dependencies_cache_identity(&file, &main, &mapped);
    let unmapped_identity = module_dependencies_cache_identity(&file, &main, &unmapped);
    assert_ne!(mapped_identity.module_map, unmapped_identity.module_map);
    assert_ne!(mapped_identity.key, unmapped_identity.key);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));

    let mapped_db = frontend_cache_database(&main, &sources, mapped, cache.clone(), false);
    let mapped_dependencies = mapped_db.expect_get(module_declarations_query(&mapped_db, &main));
    assert!(matches!(
        mapped_dependencies.semantic.explicit_imports[0].path,
        crate::used_paths::UsedModulePath::Package { .. }
    ));

    let unmapped_db =
        frontend_cache_database(&main, &sources, unmapped.clone(), cache.clone(), false);
    let unmapped_dependencies =
        unmapped_db.expect_get(module_declarations_query(&unmapped_db, &main));
    assert!(matches!(
        unmapped_dependencies.semantic.explicit_imports[0].path,
        crate::used_paths::UsedModulePath::Local { .. }
    ));
    assert_ne!(mapped_dependencies, unmapped_dependencies);
    assert_eq!(
        query_executions(&unmapped_db.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, unmapped, cache, false);
    let reused_dependencies = reused.expect_get(module_declarations_query(&reused, &main));
    assert_eq!(unmapped_dependencies, reused_dependencies);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn module_dependencies_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("module_dependencies_verification_replaces_wrong_valid_entry");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "pub module child;");
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let wrong = crate::used_paths::ModuleDeclarations {
        declarations: Vec::new(),
        package_roots: Vec::new(),
        used_module_paths: Vec::new(),
        explicit_imports: Vec::new(),
        used_import_aliases: Vec::new(),
    };
    cache
        .publish_module_dependencies(
            identity.namespace,
            &identity.module,
            crate::frontend_cache::ModuleDependenciesSource::new(
                identity.source,
                identity.source_len,
            ),
            identity.module_map,
            &wrong,
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid module dependencies");

    let verifying =
        frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), true);
    let verified = verifying.expect_get(module_declarations_query(&verifying, &main));
    assert_eq!(verified.semantic.declarations.len(), 1);
    assert_eq!(verified.semantic.declarations[0].name, sym("child"));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, module_map, cache, false);
    let reused_dependencies = reused.expect_get(module_declarations_query(&reused, &main));
    assert_eq!(verified, reused_dependencies);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn module_dependencies_with_diagnostics_are_not_persisted() {
    let root = temp_dir("module_dependencies_with_diagnostics_are_not_persisted");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "module child; module child;");
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let database = frontend_cache_database(&main, &sources, module_map, cache.clone(), false);
    let dependencies = database.expect_get(module_declarations_query(&database, &main));

    assert!(!dependencies.diagnostics.is_empty());
    assert!(!cache.module_dependencies_path(identity.key).is_file());

    let malformed_file = sources.set_source(main.clone(), "fn broken(");
    let malformed_identity =
        module_dependencies_cache_identity(&malformed_file, &main, &ModuleMap::new());
    let malformed =
        frontend_cache_database(&main, &sources, ModuleMap::new(), cache.clone(), false);
    let malformed_dependencies = malformed.expect_get(module_declarations_query(&malformed, &main));
    assert!(malformed_dependencies.semantic.declarations.is_empty());
    assert!(
        !cache
            .module_dependencies_path(malformed_identity.key)
            .is_file()
    );
}
