use super::*;

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) struct CheckedFixture {
    pub(super) module_id: ModuleId,
    pub(super) type_store: TypeStore,
    pub(super) defs: DefCollection,
    pub(super) locals: LocalResolution,
    pub(super) const_module: ConstModuleLowering,
    pub(super) checked: ConstCheck,
}

pub(super) fn check_source(source: &str) -> CheckedFixture {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_names = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let item_tree = ModuleItemTree::from_module(&module);
    let type_store = TypeStore::new();
    let lowered = lower_module_types_from_item_tree_with_context(
        module_id,
        &item_tree,
        &type_names,
        TypeLoweringContext::empty(&type_store),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &lowered,
        type_store: &type_store,
        symbols: None,
    });
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses =
        semantic_use_table(module_id, &values, &locals, &lowered, &active_item_tree);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-const-check-test/main.nia");
    let const_module = lower_module_const(ConstModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &lowered.const_exprs,
        source_path: &source_path,
    });
    let input = ConstInput {
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &lowered,
        signatures: &signatures,
        type_store: &type_store,
        normalization: &nia_type_normalize::TypeNormalization {
            normalized: HashMap::new(),
            diagnostics: Vec::new(),
        },
        target: &target,
        source_path: &source_path,
        program: ConstProgramContext::empty(),
    };
    let checked = check_module_const(input);
    CheckedFixture {
        module_id,
        type_store,
        defs,
        locals,
        const_module,
        checked,
    }
}

pub(super) fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    active_item_tree: &ActiveModuleItemTree,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder
        .extend_node_type_uses(lowered.versioned_type_uses_from_active_item_tree(active_item_tree));
    builder.finish()
}
