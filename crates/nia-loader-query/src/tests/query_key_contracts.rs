use super::*;

#[test]
fn source_frontend_query_keys_are_compact_handles() {
    assert_eq!(std::mem::size_of::<ProviderDemandsQuery>(), 0);
    assert_eq!(
        std::mem::size_of::<crate::graph::ModuleGraphRevisionQuery>(),
        16
    );
    assert_eq!(std::mem::size_of::<SourceTextQuery>(), 4);
    assert_eq!(std::mem::size_of::<SourceStatusQuery>(), 4);
    assert_eq!(std::mem::size_of::<LoadedModuleQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleOriginsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleParseErrorsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleItemTreeFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ActiveModuleItemTreeFactQuery>(), 8);
    assert_eq!(std::mem::size_of::<ParsedModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<SyntaxModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleDeclarationsQuery>(), 16);
    assert_eq!(std::mem::size_of::<ProviderSummaryQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleFacadeFactsQuery>(), 16);
    assert_eq!(std::mem::size_of::<PublicSurfaceModuleFactsQuery>(), 16);
}
