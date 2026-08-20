use super::*;
use nia_ids::ModuleIdAllocator;

fn const_arg(ty: InternedTyId, value: u128) -> ConstGenericArg {
    ConstGenericArg {
        ty,
        value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(value)),
    }
}

fn const_param(ty: InternedTyId, name: SymbolId) -> ConstGenericArg {
    ConstGenericArg {
        ty,
        value: ConstGenericValue::GenericParam(name),
    }
}

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

#[test]
fn trait_object_impl_matching_checks_binding_value_before_committing_candidate() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.primitive(PrimitiveTy::I32);
    let bool_ty = append.primitive(PrimitiveTy::Bool);
    let object_trait = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(1),
    });
    let obligation_trait = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(2),
    });
    let item = SymbolId::from_stable_hash(20);
    let binding = |ty| nia_ty::AssociatedTypeBindingTy {
        name: item,
        trait_id: None,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        ty,
    };
    let pattern_ty = append.intern(TyKind::TraitObject {
        is_readonly: false,
        trait_id: object_trait,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: vec![binding(i32_ty), binding(bool_ty)],
    });
    let actual_ty = append.intern(TyKind::TraitObject {
        is_readonly: false,
        trait_id: object_trait,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        associated_type_bindings: vec![binding(bool_ty), binding(i32_ty)],
    });
    let trait_impls = vec![ProgramTraitImplSignature {
        module_id,
        impl_id: TraitImplId(20),
        builtin: None,
        generics: Vec::new(),
        generic_params: Vec::new(),
        target_ty: pattern_ty,
        trait_id: obligation_trait,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        where_predicates: Vec::new(),
        associated_types: Vec::new(),
        associated_values: Vec::new(),
    }];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    let selection = solver.match_user_impl_at(
        &TraitGoal {
            self_ty: actual_ty,
            trait_id: obligation_trait,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
        },
        0,
    );
    assert!(
        selection.is_some(),
        "a later compatible associated binding must be considered after an incompatible key match"
    );
}

#[test]
fn callable_pointees_are_unsized_while_callable_views_are_sized() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.primitive(PrimitiveTy::I32);
    let pointee = append.intern(TyKind::CallablePointee {
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let view = append.intern(TyKind::Callable {
        is_readonly: true,
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let trait_impls = Vec::new();
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);
    let goal = |self_ty, builtin| TraitGoal {
        self_ty,
        trait_id: TraitId::Builtin(builtin),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
    };

    assert!(solver.proves(goal(view, BuiltinTrait::Sized)));
    assert!(!solver.proves(goal(pointee, BuiltinTrait::Sized)));
    assert!(solver.proves(goal(pointee, BuiltinTrait::Unsized)));
}

#[test]
fn concrete_closure_states_are_sized_when_their_captures_are_sized() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.primitive(PrimitiveTy::I32);
    let closure = append.intern(TyKind::ClosureState {
        closure_id: nia_ids::ClosureId {
            owner: GlobalDefId {
                module_id,
                def_id: DefId(1),
            },
            ordinal: 0,
        },
        captures: vec![i32_ty],
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let trait_impls = Vec::new();
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert!(solver.proves(TraitGoal {
        self_ty: closure,
        trait_id: TraitId::Builtin(BuiltinTrait::Sized),
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
    }));
}

#[test]
fn associated_type_substitutes_const_arguments_inferred_from_impl_target() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.primitive(PrimitiveTy::Usize);
    let u8_ty = append.primitive(PrimitiveTy::U8);
    let n = SymbolId::from_stable_hash(10);
    let item = SymbolId::from_stable_hash(11);
    let container = GlobalDefId {
        module_id,
        def_id: DefId(10),
    };
    let trait_id = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(11),
    });
    let pattern_ty = append.intern(TyKind::Nominal {
        def_id: container,
        args: Vec::new(),
        const_args: vec![const_param(usize_ty, n)],
    });
    let actual_ty = append.intern(TyKind::Nominal {
        def_id: container,
        args: Vec::new(),
        const_args: vec![const_arg(usize_ty, 3)],
    });
    let associated_ty = append.intern(TyKind::Array {
        len: ArrayLenTy::GenericParam(n),
        elem: u8_ty,
    });
    let trait_impls = vec![ProgramTraitImplSignature {
        module_id,
        impl_id: TraitImplId(10),
        builtin: None,
        generics: vec![n],
        generic_params: vec![nia_item_signatures::GenericParamSignature {
            name: n,
            kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
        }],
        target_ty: pattern_ty,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        where_predicates: Vec::new(),
        associated_types: vec![nia_item_signatures::TraitImplAssociatedTypeSignature {
            name: item,
            ty: associated_ty,
            span: nia_span::Span::default(),
        }],
        associated_values: Vec::new(),
    }];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    let missing_item = SymbolId::from_stable_hash(12);
    let mut active = HashSet::new();
    assert_eq!(
        solver.resolve_associated_type_inner(
            actual_ty,
            trait_id,
            &[],
            &[],
            &missing_item,
            &mut active,
        ),
        None
    );
    assert!(
        active.is_empty(),
        "missing associated items must release the projection cycle guard"
    );

    let resolved = solver
        .resolve_associated_type(actual_ty, trait_id, &[], &[], &item)
        .expect("associated type should resolve");
    assert!(matches!(
        type_store.get(resolved),
        Some(TyKind::Array {
            len: ArrayLenTy::ConstValue(3),
            elem,
        }) if *elem == u8_ty
    ));
}

#[test]
fn impl_where_predicate_substitutes_const_arguments_from_target() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.primitive(PrimitiveTy::Usize);
    let n = SymbolId::from_stable_hash(20);
    let marker = GlobalDefId {
        module_id,
        def_id: DefId(20),
    };
    let container = GlobalDefId {
        module_id,
        def_id: DefId(21),
    };
    let prerequisite = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(22),
    });
    let outer = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(23),
    });
    let marker_three = append.intern(TyKind::Nominal {
        def_id: marker,
        args: Vec::new(),
        const_args: vec![const_arg(usize_ty, 3)],
    });
    let marker_n = append.intern(TyKind::Nominal {
        def_id: marker,
        args: Vec::new(),
        const_args: vec![const_param(usize_ty, n)],
    });
    let container_n = append.intern(TyKind::Nominal {
        def_id: container,
        args: Vec::new(),
        const_args: vec![const_param(usize_ty, n)],
    });
    let container_three = append.intern(TyKind::Nominal {
        def_id: container,
        args: Vec::new(),
        const_args: vec![const_arg(usize_ty, 3)],
    });
    let prerequisite_ty = append.intern(TyKind::Nominal {
        def_id: match prerequisite {
            TraitId::Source(def_id) => def_id,
            TraitId::Builtin(_) => unreachable!(),
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let trait_impls = vec![
        ProgramTraitImplSignature {
            module_id,
            impl_id: TraitImplId(20),
            builtin: None,
            generics: Vec::new(),
            generic_params: Vec::new(),
            target_ty: marker_three,
            trait_id: prerequisite,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            where_predicates: Vec::new(),
            associated_types: Vec::new(),
            associated_values: Vec::new(),
        },
        ProgramTraitImplSignature {
            module_id,
            impl_id: TraitImplId(21),
            builtin: None,
            generics: vec![n],
            generic_params: vec![nia_item_signatures::GenericParamSignature {
                name: n,
                kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
            }],
            target_ty: container_n,
            trait_id: outer,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            where_predicates: vec![nia_item_signatures::WherePredicateSignature {
                ty: marker_n,
                bounds: vec![nia_item_signatures::WhereBoundSignature {
                    trait_ty: prerequisite_ty,
                    associated_type_bindings: Vec::new(),
                    span: nia_span::Span::default(),
                }],
                span: nia_span::Span::default(),
            }],
            associated_types: Vec::new(),
            associated_values: Vec::new(),
        },
    ];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert!(matches!(
        solver.select_user_impl(TraitGoal {
            self_ty: container_three,
            trait_id: outer,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
        }),
        TraitSelection::User(UserImpl { impl_index: 1, .. })
    ));
}

#[test]
fn concrete_trait_const_argument_is_more_specific_than_a_parameter() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.primitive(PrimitiveTy::Usize);
    let n = SymbolId::from_stable_hash(30);
    let target = append.intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(30),
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let trait_id = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(31),
    });
    let make_impl =
        |impl_id, generics, generic_params, trait_const_args| ProgramTraitImplSignature {
            module_id,
            impl_id,
            builtin: None,
            generics,
            generic_params,
            target_ty: target,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args,
            where_predicates: Vec::new(),
            associated_types: Vec::new(),
            associated_values: Vec::new(),
        };
    let trait_impls = vec![
        make_impl(
            TraitImplId(30),
            vec![n],
            vec![nia_item_signatures::GenericParamSignature {
                name: n,
                kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
            }],
            vec![const_param(usize_ty, n)],
        ),
        make_impl(
            TraitImplId(31),
            Vec::new(),
            Vec::new(),
            vec![const_arg(usize_ty, 3)],
        ),
    ];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert!(matches!(
        solver.select_user_impl(TraitGoal {
            self_ty: target,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args: vec![const_arg(usize_ty, 3)],
        }),
        TraitSelection::User(UserImpl { impl_index: 1, .. })
    ));
}

#[test]
fn repeated_type_parameter_across_impl_header_is_more_specific() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.primitive(PrimitiveTy::I32);
    let a = SymbolId::from_stable_hash(32);
    let b = SymbolId::from_stable_hash(33);
    let t = SymbolId::from_stable_hash(34);
    let a_ty = append.intern(TyKind::GenericParam(a));
    let b_ty = append.intern(TyKind::GenericParam(b));
    let t_ty = append.intern(TyKind::GenericParam(t));
    let trait_id = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(32),
    });
    let type_param = |name| nia_item_signatures::GenericParamSignature {
        name,
        kind: nia_item_signatures::GenericParamSignatureKind::Type,
    };
    let make_impl =
        |impl_id, generics: Vec<SymbolId>, target_ty, trait_args| ProgramTraitImplSignature {
            module_id,
            impl_id,
            builtin: None,
            generic_params: generics.iter().copied().map(type_param).collect(),
            generics,
            target_ty,
            trait_id,
            trait_args,
            trait_const_args: Vec::new(),
            where_predicates: Vec::new(),
            associated_types: Vec::new(),
            associated_values: Vec::new(),
        };
    let trait_impls = vec![
        make_impl(TraitImplId(32), vec![a, b], a_ty, vec![b_ty]),
        make_impl(TraitImplId(33), vec![t], t_ty, vec![t_ty]),
    ];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert!(matches!(
        solver.select_user_impl(TraitGoal {
            self_ty: i32_ty,
            trait_id,
            trait_args: vec![i32_ty],
            trait_const_args: Vec::new(),
        }),
        TraitSelection::User(UserImpl { impl_index: 1, .. })
    ));
}

#[test]
fn repeated_const_parameter_across_impl_header_is_more_specific() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.primitive(PrimitiveTy::Usize);
    let a = SymbolId::from_stable_hash(35);
    let b = SymbolId::from_stable_hash(36);
    let n = SymbolId::from_stable_hash(37);
    let container = GlobalDefId {
        module_id,
        def_id: DefId(35),
    };
    let trait_id = TraitId::Source(GlobalDefId {
        module_id,
        def_id: DefId(36),
    });
    let container_with = |arg| {
        append.intern(TyKind::Nominal {
            def_id: container,
            args: Vec::new(),
            const_args: vec![arg],
        })
    };
    let const_param_signature = |name| nia_item_signatures::GenericParamSignature {
        name,
        kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
    };
    let make_impl =
        |impl_id, generics: Vec<SymbolId>, target_ty, trait_const_args| ProgramTraitImplSignature {
            module_id,
            impl_id,
            builtin: None,
            generic_params: generics
                .iter()
                .copied()
                .map(const_param_signature)
                .collect(),
            generics,
            target_ty,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args,
            where_predicates: Vec::new(),
            associated_types: Vec::new(),
            associated_values: Vec::new(),
        };
    let trait_impls = vec![
        make_impl(
            TraitImplId(35),
            vec![a, b],
            container_with(const_param(usize_ty, a)),
            vec![const_param(usize_ty, b)],
        ),
        make_impl(
            TraitImplId(36),
            vec![n],
            container_with(const_param(usize_ty, n)),
            vec![const_param(usize_ty, n)],
        ),
    ];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert!(matches!(
        solver.select_user_impl(TraitGoal {
            self_ty: container_with(const_arg(usize_ty, 3)),
            trait_id,
            trait_args: Vec::new(),
            trait_const_args: vec![const_arg(usize_ty, 3)],
        }),
        TraitSelection::User(UserImpl { impl_index: 1, .. })
    ));
}

#[test]
fn cyclic_impl_where_predicates_do_not_prove_each_other() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let target = append.intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id,
            def_id: DefId(40),
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let trait_a_def = GlobalDefId {
        module_id,
        def_id: DefId(41),
    };
    let trait_b_def = GlobalDefId {
        module_id,
        def_id: DefId(42),
    };
    let trait_a = TraitId::Source(trait_a_def);
    let trait_b = TraitId::Source(trait_b_def);
    let trait_a_ty = append.intern(TyKind::Nominal {
        def_id: trait_a_def,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let trait_b_ty = append.intern(TyKind::Nominal {
        def_id: trait_b_def,
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let where_bound = |trait_ty| nia_item_signatures::WherePredicateSignature {
        ty: target,
        bounds: vec![nia_item_signatures::WhereBoundSignature {
            trait_ty,
            associated_type_bindings: Vec::new(),
            span: nia_span::Span::default(),
        }],
        span: nia_span::Span::default(),
    };
    let make_impl = |impl_id, trait_id, required| ProgramTraitImplSignature {
        module_id,
        impl_id,
        builtin: None,
        generics: Vec::new(),
        generic_params: Vec::new(),
        target_ty: target,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        where_predicates: vec![where_bound(required)],
        associated_types: Vec::new(),
        associated_values: Vec::new(),
    };
    let trait_impls = vec![
        make_impl(TraitImplId(40), trait_a, trait_b_ty),
        make_impl(TraitImplId(41), trait_b, trait_a_ty),
    ];
    let normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let local_enums = HashMap::new();
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
    let mut solver = context.solver(&[]);

    assert_eq!(
        solver.resolve(TraitGoal {
            self_ty: target,
            trait_id: trait_a,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
        }),
        TraitResolution::Unsatisfied
    );
}
