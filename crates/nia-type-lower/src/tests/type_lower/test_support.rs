fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(nia_symbol::stable_hash(text))
}

fn lower_test_module(
    module: &Module,
    defs: &DefCollection,
    resolved: &TypeResolution,
) -> (nia_ty::TypeStore, TypeLowering) {
    let module_id = defs.module_id;
    let program_defs = HashMap::from([(module_id, Arc::new(defs.clone()))]);
    let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let type_store = nia_ty::TypeStore::new();
    let lowered = lower_module_types_with_context(
        module_id,
        module,
        resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        ),
    );
    (type_store, lowered)
}
