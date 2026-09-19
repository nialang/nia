use super::*;

#[test]
fn rejects_lowered_types_from_another_type_store() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, errors) = parse_module("fn id(value: i32) i32 { value }");
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let resolved = resolve_module_types(&module, &defs);
    let lowering_store = TypeStore::new().expect("create type store");
    let lowering = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::empty(&lowering_store),
    )
    .expect("lower module types");
    let signature_store = TypeStore::new().expect("create type store");
    let error = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowering,
        type_store: &signature_store,
        symbols: None,
    })
    .expect_err("foreign type store IDs must be rejected as an internal error");

    assert!(error.message.contains("outside the session type store"));
}
