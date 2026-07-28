use super::*;

#[test]
fn effective_generics_cache_uses_recorded_generics_by_reference() {
    let (module_id, mut collector) = empty_collector();
    let def_id = GlobalDefId {
        module_id,
        def_id: nia_ids::DefId(0),
    };
    collector
        .recorded_generics_by_def
        .insert(def_id, vec![sym("T"), sym("U")]);

    assert_eq!(
        collector.effective_generics_for(def_id),
        &[sym("T"), sym("U")]
    );
    collector.recorded_generics_by_def.clear();
    assert_eq!(
        collector.effective_generics_for(def_id),
        &[sym("T"), sym("U")]
    );
}

#[test]
fn ordered_type_substitutions_reuse_existing_ids() {
    let (module_id, mut collector) = empty_collector();
    let append = collector.type_store.append_for_module(module_id);
    let i32_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::I32));
    let bool_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::Bool));

    let first = collector
        .intern_ordered_type_substitutions(None, vec![(sym("T"), i32_ty), (sym("U"), bool_ty)]);
    let second = collector
        .intern_ordered_type_substitutions(None, vec![(sym("T"), i32_ty), (sym("U"), bool_ty)]);

    assert_eq!(first, second);
    assert_eq!(collector.type_substitutions.len(), 1);
    assert_eq!(collector.type_substitutions[first.0].self_arg, None);
    assert_eq!(
        collector.type_substitutions[first.0].substitutions,
        [(sym("T"), i32_ty), (sym("U"), bool_ty)]
            .into_iter()
            .collect::<SymbolMap<_>>()
    );
}

fn empty_collector() -> (ModuleId, MonoCollector<'static>) {
    static FIXTURE: std::sync::LazyLock<(TypeStore, ModuleId)> = std::sync::LazyLock::new(|| {
        let mut module_ids = ModuleIdAllocator::new();
        (TypeStore::new(), module_ids.allocate())
    });
    let (type_store, module_id) = &*FIXTURE;
    let collector = MonoCollector {
        type_store,
        defs_by_module: HashMap::new(),
        normalizations_by_module: HashMap::new(),
        const_by_module: HashMap::new(),
        const_expr_summaries_by_module: HashMap::new(),
        layouts_by_module: HashMap::new(),
        local_enums_by_module: HashMap::new(),
        program_enums: &EMPTY_PROGRAM_ENUMS,
        trait_impls: &[],
        trait_impl_index: &EMPTY_PROGRAM_TRAIT_IMPL_INDEX,
        instantiations_by_source: HashMap::new(),
        source_instantiation_edges: Vec::new(),
        recorded_generics_by_def: HashMap::new(),
        instances: Vec::new(),
        seen: HashSet::new(),
        type_symbols: HashMap::new(),
        def_names: HashMap::new(),
        base_symbols: HashMap::new(),
        type_instantiations: HashMap::new(),
        type_substitutions: Vec::new(),
        type_substitution_ids: HashMap::new(),
        effective_generics: HashMap::new(),
        missing_array_len_diagnostics: HashSet::new(),
        diagnostics: Vec::new(),
    };
    (*module_id, collector)
}
