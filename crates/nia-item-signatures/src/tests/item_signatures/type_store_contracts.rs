use super::*;

#[test]
fn rejects_lowered_types_from_another_type_store() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, errors) = parse_module("fn id(value: i32) i32 { value }");
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types(&module, &defs);
    let lowering_store = TypeStore::new();
    let lowering = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::empty(&lowering_store),
    );
    let signature_store = TypeStore::new();
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowering,
        type_store: &signature_store,
        symbols: None,
    });

    assert!(signatures.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("outside the session type store")
    }));
}
