use super::*;
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};

#[test]
fn facade_facts_cache_round_trips_all_path_processing_modes() {
    let root = temp_dir("facade_facts_cache_round_trips_all_path_processing_modes");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() () {}");
    let facade_file = sources.set_source(facade, "pub struct Widget {}");
    let module_map = ModuleMap::new();
    let identity = facade_cache_identity(&facade_file, &main, &module_map);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let symbol_texts = [
        "Widget", "TraitA", "TraitB", "first", "second", "dep", "segment0", "segment1", "segment2",
        "segment3", "segment4", "segment5", "segment6", "segment7",
    ];
    let symbols = symbols_for(&symbol_texts);
    let processing = [
        UsedModulePathProcessing::Never,
        UsedModulePathProcessing::Always,
        UsedModulePathProcessing::IfSelectedItem,
        UsedModulePathProcessing::IfProvidesExtensions,
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name: None,
            trait_name: sym("TraitA"),
        },
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl {
            trait_name: sym("TraitB"),
        },
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: None,
            associated_name: sym("first"),
        },
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: Some(sym("Widget")),
            associated_name: sym("second"),
        },
    ];
    let mut paths = processing
        .into_iter()
        .enumerate()
        .map(|(index, processing)| {
            let segments = vec![sym(symbol_texts[index + 6])];
            match index % 4 {
                0 => UsedModulePath::Package {
                    package: sym("dep"),
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                1 => UsedModulePath::PackageRelative {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                2 => UsedModulePath::ParentRelative {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                _ => UsedModulePath::Local {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
            }
        })
        .collect::<Vec<_>>();
    paths.sort();
    let facts = crate::facade_facts::ModuleFacadeFacts::from_cache_parts(
        [sym("Widget")],
        Vec::new(),
        paths,
    );
    cache
        .publish_facade_facts(
            identity.namespace,
            &identity.module,
            identity.item_signature,
            identity.module_map,
            &facts,
            &symbols,
        )
        .expect("publish facade facts path variants");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_facade_facts(
                identity.facade_key,
                identity.namespace,
                &identity.module,
                identity.item_signature,
                identity.module_map,
                &loaded_symbols,
            )
            .expect("load facade facts path variants"),
        crate::frontend_cache::FacadeFactsCacheLookup::Hit(cached) if cached == facts
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("segment7")).as_deref(),
        Some("segment7")
    );
}

#[test]
fn facade_facts_are_cached_for_reexport_and_provider_loading() {
    let root = temp_dir("facade_facts_are_cached_for_reexport_and_provider_loading");
    let main = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main,
        r#"
using dep::facade;

fn first(value: facade::Widget) i32 {
    value.score()
}

fn second(value: facade::Widget) i32 {
    value.score()
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("facade")).expect("create facade dir");
    write(
        &root.join("facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

using self::providers;
pub using types::Widget;
"#,
    );
    write(
        &root.join("facade/types.nia"),
        "pub struct Widget { value: i32 }",
    );
    write(
        &root.join("facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
    pub fn score(&self) i32 { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let entry_path = SourcePath::new(main.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(main.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    database
        .update_provider_demands([ProviderDemand {
            source_path: entry_path,
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(sym("Widget")),
                method_name: sym("score"),
            },
        }])
        .expect("provider graph update");
    let program = database.load_program().expect("provider program load");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("facade/providers.nia").to_string_lossy().as_ref(),
    );

    let trace = database.query_trace();
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "module_facade_facts")
        .expect("facade facts query should be recorded for custom package facade");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert!(
        query.stats.cache_hits >= 1,
        "reexport and provider loading should reuse facade facts: {query:?}"
    );
}
