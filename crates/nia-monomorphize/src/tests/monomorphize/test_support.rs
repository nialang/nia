fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn value_def(defs: &DefCollection, name: &str) -> DefId {
    defs.module_scope.values.get(&sym(name)).expect("value def")
}

struct TestTypes {
    append: TypeStoreAppend,
}

struct TestFixture {
    module_id: ModuleId,
    type_store: TypeStore,
    types: TestTypes,
}

impl TestTypes {
    fn intern(&self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }

    fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.intern(TyKind::Primitive(primitive))
    }
}

fn generic_param(types: &TestTypes, name: &str) -> InternedTyId {
    types.intern(TyKind::GenericParam(sym(name)))
}

fn inst(
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    span: Span,
    source_def_id: Option<GlobalDefId>,
) -> GenericInstantiation {
    GenericInstantiation {
        def_id,
        self_arg: None,
        args,
        const_args: Vec::new(),
        generics: vec![sym("T")],
        span,
        source_def_id,
    }
}

fn normalization_for() -> TypeNormalization {
    TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    }
}

fn test_fixture() -> TestFixture {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let types = TestTypes {
        append: type_store.append_for_module(module_id),
    };
    TestFixture {
        module_id,
        type_store,
        types,
    }
}

fn collect_test_monomorphizations(
    inputs: &[MonomorphizeModuleInput<'_>],
    type_store: &TypeStore,
) -> Monomorphization {
    collect_monomorphizations(
        inputs,
        inputs
            .iter()
            .map(|input| (input.module_id, input.source_identity.clone())),
        type_store,
    )
}

fn mono_input<'a>(
    defs: &'a DefCollection,
    normalization: &'a TypeNormalization,
    const_eval: &'a ConstCheck,
    const_expr_summaries: &'a HashMap<GlobalConstExprId, ConstExprSummary>,
    instantiations: &'a [GenericInstantiation],
) -> MonomorphizeModuleInput<'a> {
    MonomorphizeModuleInput {
        module_id: defs.module_id,
        source_identity: nia_source::SourceIdentity::new(format!(
            "test/module-{}.nia",
            defs.module_id.local_index()
        )),
        defs,
        normalization,
        const_eval,
        const_expr_summaries,
        layouts: None,
        local_enums: &EMPTY_LOCAL_ENUMS,
        program_enums: &EMPTY_PROGRAM_ENUMS,
        trait_impls: &[],
        trait_impl_index: &EMPTY_PROGRAM_TRAIT_IMPL_INDEX,
        instantiations,
    }
}
