use super::*;

fn wide_query_key_size() -> usize {
    if cfg!(target_pointer_width = "64") {
        16
    } else {
        12
    }
}

#[test]
fn source_frontend_query_keys_are_compact_handles() {
    assert_eq!(std::mem::size_of::<ProviderDemandsQuery>(), 0);
    assert_eq!(
        std::mem::size_of::<crate::graph::ModuleGraphRevisionQuery>(),
        std::mem::size_of::<nia_compiler_query::ProviderFactRevision>()
    );
    assert!(std::mem::size_of::<nia_compiler_query::ProviderFactRevision>() <= 16);
    assert_eq!(std::mem::size_of::<SourceTextQuery>(), 4);
    assert_eq!(std::mem::size_of::<SourceStatusQuery>(), 4);
    assert_eq!(std::mem::size_of::<LoadedModuleQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleOriginsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleParseErrorsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleItemTreeFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ActiveModuleItemTreeFactQuery>(), 8);
    assert_eq!(
        std::mem::size_of::<ParsedModuleQuery>(),
        wide_query_key_size()
    );
    assert_eq!(
        std::mem::size_of::<SyntaxModuleQuery>(),
        wide_query_key_size()
    );
    assert_eq!(
        std::mem::size_of::<ModuleDeclarationsQuery>(),
        wide_query_key_size()
    );
    assert_eq!(
        std::mem::size_of::<ProviderSummaryQuery>(),
        wide_query_key_size()
    );
    assert_eq!(
        std::mem::size_of::<ModuleFacadeFactsQuery>(),
        wide_query_key_size()
    );
    assert_eq!(
        std::mem::size_of::<PublicSurfaceModuleFactsQuery>(),
        wide_query_key_size()
    );
}
