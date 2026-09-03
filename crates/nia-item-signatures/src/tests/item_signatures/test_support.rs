fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn signatures_ok(source: &str) -> ItemSignatures {
    let signatures = signatures(source);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    signatures
}

fn signatures(source: &str) -> ItemSignatures {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = collect_module_defs(module_id, &module);
    assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
    let resolved = resolve_module_types(&module, &defs);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let type_store = TypeStore::new();
    let program_defs = HashMap::from([(module_id, Arc::new(defs.clone()))]);
    let defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let lowered = lower_module_types_with_context(
        module_id,
        &module,
        &resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&defs_by_module),
            },
        ),
    );
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowered,
        type_store: &type_store,
        symbols: None,
    })
}

struct BoolResolver(bool);

impl nia_item_tree::ConditionResolver for BoolResolver {
    fn resolve_condition(
        &mut self,
        cond: &nia_ast::ConditionExpr,
    ) -> Result<bool, nia_item_tree::ItemTreeError> {
        match &cond.kind {
            nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
            _ => Ok(self.0),
        }
    }
}
