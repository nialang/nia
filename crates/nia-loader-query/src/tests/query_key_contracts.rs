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
    let source_id_size = std::mem::size_of::<SourceId>();
    assert_eq!(source_id_size, 8);
    assert_eq!(std::mem::size_of::<SourceTextQuery>(), source_id_size);
    assert_eq!(std::mem::size_of::<SourceStatusQuery>(), source_id_size);
    assert_eq!(std::mem::size_of::<LoadedModuleQuery>(), source_id_size);
    assert_eq!(
        std::mem::size_of::<ModuleOriginsFactQuery>(),
        source_id_size
    );
    assert_eq!(
        std::mem::size_of::<ModuleParseErrorsFactQuery>(),
        source_id_size
    );
    assert_eq!(
        std::mem::size_of::<ModuleItemTreeFactQuery>(),
        source_id_size
    );
    let active_unaligned =
        source_id_size + std::mem::size_of::<nia_compiler_query::ActiveModuleItemTreeFactKind>();
    let source_id_alignment = std::mem::align_of::<SourceId>();
    let active_size = active_unaligned.next_multiple_of(source_id_alignment);
    assert_eq!(
        std::mem::size_of::<ActiveModuleItemTreeFactQuery>(),
        active_size
    );
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
