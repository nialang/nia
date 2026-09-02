fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn compute_test_const(
    module_id: ModuleId,
    type_store: &TypeStore,
    module: &nia_ast::Module,
    symbols: &SymbolTable,
    defs: &nia_defs::DefCollection,
    signatures: &ItemSignatures,
    lowered: &nia_type_lower::TypeLowering,
) -> ConstCheck {
    let values = resolve_module_values(module, defs);
    let locals = resolve_module_locals(module, defs, &values);
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree =
        ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default());
    let semantic_uses =
        semantic_use_table(module_id, &values, &locals, lowered, &active_item_tree);
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-layout-test/main.nia");
    let const_module = lower_module_const(ConstModuleInput {
        active_item_tree: &active_item_tree,
        defs,
        signatures,
        type_store,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols,
        const_exprs: &lowered.const_exprs,
        source_path: &source_path,
    });
    assert!(
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let input = ConstInput {
        type_store,
        module: &const_module.module,
        defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols,
        lowered,
        signatures,
        normalization: &nia_const_check::TypeNormalization {
            normalized: HashMap::new(),
            diagnostics: Vec::new(),
        },
        target: &target,
        source_path: &source_path,
        program: ConstProgramContext::empty(),
    };
    check_module_const(input)
}

fn lower_test_module(
    module: &nia_ast::Module,
    resolved: &nia_type_resolve::TypeResolution,
    defs: &nia_defs::DefCollection,
) -> (TypeStore, nia_type_lower::TypeLowering) {
    let module_id = defs.module_id;
    let type_store = TypeStore::new();
    let program_defs =
        std::collections::HashMap::from([(module_id, std::sync::Arc::new(defs.clone()))]);
    let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let lowered = lower_module_types_with_context(
        module_id,
        module,
        resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        ),
    );
    (type_store, lowered)
}

fn collect_test_signatures(
    module: &nia_ast::Module,
    defs: &nia_defs::DefCollection,
    lowered: &nia_type_lower::TypeLowering,
    type_store: &TypeStore,
) -> ItemSignatures {
    collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(module),
        defs,
        lowered,
        type_store,
        symbols: None,
    })
}

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    lowered: &nia_type_lower::TypeLowering,
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
                    nia_ids::GlobalDefId {
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
    builder.extend_node_type_uses(
        lowered.versioned_type_uses_from_active_item_tree(active_item_tree),
    );
    builder.finish()
}

fn parse_test_module(source: &str) -> (nia_ast::Module, SymbolTable) {
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    (module, symbols)
}
