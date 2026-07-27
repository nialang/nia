use super::*;
use crate::used_paths::{ExplicitUsingImport, UsedModulePath, UsedModulePathProcessing};

#[test]
fn module_dependencies_cache_round_trips_all_stable_fields() {
    let root = temp_dir("module_dependencies_cache_round_trips_all_stable_fields");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), " ".repeat(256));
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let symbols = symbols_for(&[
        "private", "super", "package", "public", "dep_b", "dep_a", "one", "two", "three", "four",
        "Trait", "Alias", "value", "AliasB", "AliasA",
    ]);
    let declarations = [
        ("private", Visibility::Private, nia_span::Span::new(1, 2)),
        ("super", Visibility::PublicSuper, nia_span::Span::new(3, 4)),
        ("package", Visibility::PublicPkg, nia_span::Span::new(5, 6)),
        ("public", Visibility::Public, nia_span::Span::new(7, 8)),
    ]
    .into_iter()
    .map(
        |(name, visibility, span)| nia_imports::ResolvedModuleDeclaration {
            name: sym(name),
            visibility,
            span,
        },
    )
    .collect::<Vec<_>>();
    let mut package_roots = vec![sym("dep_b"), sym("dep_a")];
    package_roots.sort();
    let mut used_module_paths = vec![
        UsedModulePath::Package {
            package: sym("dep_a"),
            segments: vec![sym("one")],
            include_declared_children: true,
            processing: UsedModulePathProcessing::Always,
        },
        UsedModulePath::PackageRelative {
            segments: vec![sym("two")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::IfSelectedItem,
        },
        UsedModulePath::ParentRelative {
            segments: vec![sym("three")],
            include_declared_children: true,
            processing: UsedModulePathProcessing::IfProvidesExtensions,
        },
        UsedModulePath::Local {
            segments: vec![sym("four")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::IfProvidesTraitImpl {
                trait_name: sym("Trait"),
            },
        },
    ];
    used_module_paths.sort();
    let explicit_imports = vec![ExplicitUsingImport {
        span: nia_span::Span::new(9, 20),
        alias: sym("Alias"),
        path: UsedModulePath::Local {
            segments: vec![sym("value")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::Never,
        },
    }];
    let mut used_import_aliases = vec![sym("AliasB"), sym("AliasA")];
    used_import_aliases.sort();
    let dependencies = crate::used_paths::ModuleDeclarations {
        declarations,
        package_roots,
        used_module_paths,
        explicit_imports,
        used_import_aliases,
        diagnostics: Vec::new(),
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
            &dependencies,
            &symbols,
        )
        .expect("publish complete module dependency summary");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_module_dependencies(
                identity.key,
                identity.namespace,
                &identity.module,
                crate::frontend_cache::ModuleDependenciesSource::new(
                    identity.source,
                    identity.source_len,
                ),
                identity.module_map,
                &loaded_symbols,
            )
            .expect("load complete module dependency summary"),
        crate::frontend_cache::ModuleDependenciesCacheLookup::Hit(cached)
            if cached == dependencies
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("private")).as_deref(),
        Some("private")
    );
}
