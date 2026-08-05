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

#[test]
fn user_impl_infers_const_generic_from_layout_builtin_array_length() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let u8_ty = append.intern(TyKind::Primitive(PrimitiveTy::U8));
    let const_name = SymbolId::from_stable_hash(1);
    let trait_id = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(1),
    });
    let layout_ty = append.intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(2),
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let impl_ty = append.intern(TyKind::Array {
        len: ArrayLenTy::GenericParam(const_name),
        elem: u8_ty,
    });
    let actual_ty = append.intern(TyKind::Array {
        len: ArrayLenTy::Builtin {
            builtin: nia_ty::LayoutBuiltin::Size,
            ty: layout_ty,
        },
        elem: u8_ty,
    });
    let trait_impls = vec![ProgramTraitImplSignature {
        module_id,
        impl_id: TraitImplId(1),
        builtin: None,
        generics: vec![const_name],
        generic_params: vec![nia_item_signatures::GenericParamSignature {
            name: const_name,
            kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
        }],
        target_ty: impl_ty,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        where_predicates: Vec::new(),
        associated_types: Vec::new(),
        associated_values: Vec::new(),
    }];
    let impl_index = ProgramTraitImplIndex::new(&trait_impls);
    let layouts = nia_layout::Layouts {
        target: nia_layout::TargetDataLayout::LP64,
        types: HashMap::from([(layout_ty, nia_layout::TypeLayout { size: 8, align: 4 })]),
        structs: HashMap::new(),
        unions: HashMap::new(),
        enums: HashMap::new(),
        struct_instances: HashMap::new(),
        union_instances: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
    let context = TraitSolverContext {
        type_store: &type_store,
        normalization: &normalization,
        trait_impls: &trait_impls,
        trait_impl_index: Some(&impl_index),
        layouts: Some(&layouts),
        local_module_id: module_id,
        local_enums: &local_enums,
        program_is_enum: None,
        const_expr_value: None,
        impl_is_visible: None,
    };
    let mut solver = context.solver(&[]);

    let TraitSelection::User(selection) = solver.select_user_impl(TraitGoal {
        self_ty: actual_ty,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
    }) else {
        panic!("layout builtin array length should select the user impl");
    };
    assert_eq!(
        selection.const_substitutions.get(&const_name),
        Some(&ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(8u8.into())),
        })
    );
}
