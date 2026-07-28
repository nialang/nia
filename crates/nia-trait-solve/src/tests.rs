use super::*;
use nia_ids::ModuleIdAllocator;

#[test]
fn enum_classification_reads_new_types_from_canonical_store() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let local_enum_id = DefId(7);
    let local_enum = GlobalDefId {
        module_id,
        def_id: local_enum_id,
    };
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let ty = append.intern(TyKind::Nominal {
        def_id: local_enum,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let mut local_enums = HashMap::new();
    local_enums.insert(
        local_enum_id,
        EnumSignature {
            backing_type: append.intern(TyKind::Primitive(PrimitiveTy::I32)),
            is_open: false,
            variants: Vec::new(),
            span: nia_span::Span::default(),
        },
    );
    let trait_impls = Vec::new();
    let context = TraitSolverContext {
        type_store: &type_store,
        normalization: &normalization,
        trait_impls: &trait_impls,
        trait_impl_index: None,
        layouts: None,
        local_module_id: module_id,
        local_enums: &local_enums,
        program_is_enum: None,
        const_expr_value: None,
        impl_is_visible: None,
    };
    let solver = context.solver(&[]);

    assert!(solver.is_enum(ty));
}
