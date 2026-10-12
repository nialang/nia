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
    let i32_ty = append
        .intern(TyKind::Primitive(nia_ty::PrimitiveTy::I32))
        .expect("intern i32 type");
    let bool_ty = append
        .intern(TyKind::Primitive(nia_ty::PrimitiveTy::Bool))
        .expect("intern bool type");

    let first = collector.intern_ordered_substitutions(
        None,
        vec![(sym("T"), i32_ty), (sym("U"), bool_ty)],
        Vec::new(),
    );
    let second = collector.intern_ordered_substitutions(
        None,
        vec![(sym("T"), i32_ty), (sym("U"), bool_ty)],
        Vec::new(),
    );

    assert_eq!(first, second);
    assert_eq!(collector.type_substitutions.len(), 1);
    assert_eq!(collector.type_substitutions[first.0].self_arg, None);
    assert_eq!(
        collector.type_substitutions[first.0].substitutions,
        [(sym("T"), i32_ty), (sym("U"), bool_ty)]
            .into_iter()
            .collect::<SymbolMap<_>>()
    );
    assert!(
        collector.type_substitutions[first.0]
            .const_substitutions
            .is_empty()
    );
}

#[test]
fn closure_owner_instance_is_monomorphized_with_its_type_and_const_arguments() {
    let (module_id, mut collector) = empty_collector();
    let append = collector.type_store.append_for_module(module_id);
    let generic_t = append
        .intern(TyKind::GenericParam(sym("T")))
        .expect("intern T");
    let generic_u = append
        .intern(TyKind::GenericParam(sym("U")))
        .expect("intern U");
    let generic_n = ConstGenericArg {
        ty: generic_u,
        value: ConstGenericValue::GenericParam(sym("N")),
    };
    let closure = append
        .intern(TyKind::ClosureState {
            closure_id: nia_ids::ClosureId {
                owner: GlobalDefId {
                    module_id,
                    def_id: nia_ids::DefId(0),
                },
                ordinal: 0,
            },
            owner_instance: Some(nia_ty::ClosureOwnerInstance {
                arg_module_id: module_id,
                self_arg: Some(generic_t),
                args: vec![generic_t],
                const_args: vec![generic_n],
            }),
            captures: vec![generic_t],
            params: vec![generic_t],
            return_type: generic_t,
        })
        .expect("intern generic closure");
    let i32_ty = append.primitive(PrimitiveTy::I32).expect("intern i32");
    let u64_ty = append.primitive(PrimitiveTy::U64).expect("intern u64");
    let concrete_n = ConstGenericArg {
        ty: generic_u,
        value: ConstGenericValue::Int(IntConst::unsigned(7)),
    };
    let substitutions = collector.intern_ordered_substitutions(
        None,
        vec![(sym("T"), i32_ty), (sym("U"), u64_ty)],
        vec![(sym("N"), concrete_n)],
    );

    let instantiated = collector.instantiate_ty(module_id, closure, substitutions);
    let Some(TyKind::ClosureState {
        owner_instance: Some(owner),
        captures,
        params,
        return_type,
        ..
    }) = collector.type_kind(instantiated)
    else {
        panic!("expected instantiated closure state")
    };

    assert_eq!(owner.self_arg, Some(i32_ty));
    assert_eq!(owner.args, vec![i32_ty]);
    assert_eq!(owner.const_args[0].ty, u64_ty);
    assert_eq!(
        owner.const_args[0].value,
        ConstGenericValue::Int(IntConst::unsigned(7))
    );
    assert_eq!(captures, vec![i32_ty]);
    assert_eq!(params, vec![i32_ty]);
    assert_eq!(return_type, i32_ty);
}

fn empty_collector() -> (ModuleId, MonoCollector<'static>) {
    static FIXTURE: std::sync::LazyLock<(TypeStore, ModuleId)> = std::sync::LazyLock::new(|| {
        let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
        (
            TypeStore::new().expect("create type store"),
            module_ids.allocate().expect("allocate module ID"),
        )
    });
    let (type_store, module_id) = &*FIXTURE;
    let collector = MonoCollector {
        type_store,
        source_identities: HashMap::from([(
            *module_id,
            nia_source::SourceIdentity::new("test/collector.nia"),
        )]),
        symbol_package_identities: HashMap::from([(*module_id, "test/package@0".into())]),
        defs_by_module: HashMap::new(),
        generic_params_by_def: HashMap::new(),
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
        type_instantiations: HashMap::new(),
        type_substitutions: Vec::new(),
        type_substitution_ids: HashMap::new(),
        effective_generics: HashMap::new(),
        effective_const_generics: HashMap::new(),
        missing_array_len_diagnostics: HashSet::new(),
        missing_source_identity_diagnostics: HashSet::new(),
        diagnostics: Vec::new(),
        internal_error: None,
    };
    (*module_id, collector)
}
