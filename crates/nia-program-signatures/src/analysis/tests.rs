// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::ModuleUsingScope;
use nia_imports::ModuleGraph;
use nia_source::{SourceId, SourcePath, SourceRevision, SourceVersion};
use nia_span::Span;
use nia_symbol::stable_hash;
use std::{collections::HashMap, sync::Arc};

trait TestTypeStoreAppend {
    fn test_intern(&self, kind: TyKind) -> InternedTyId;
    fn test_primitive(&self, primitive: PrimitiveTy) -> InternedTyId;
}

impl TestTypeStoreAppend for nia_ty::TypeStoreAppend {
    fn test_intern(&self, kind: TyKind) -> InternedTyId {
        self.intern(kind)
            .expect("intern program-signatures test type")
    }

    fn test_primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.primitive(primitive)
            .expect("intern primitive program-signatures test type")
    }
}

thread_local! {
    static TEST_SYMBOLS: nia_symbol_table::SymbolTable = nia_symbol_table::SymbolTable::new();
}

fn test_symbols() -> nia_symbol_table::SymbolTable {
    TEST_SYMBOLS.with(Clone::clone)
}

fn sym(text: &str) -> SymbolId {
    test_symbols()
        .intern(text)
        .unwrap_or_else(|error| panic!("test symbol collision: {error}"));
    SymbolId::from_stable_hash(stable_hash(text))
}

fn intern_child(
    graph: &mut ModuleGraph,
    parent: nia_ids::ModuleId,
    child_name: &str,
    visibility: nia_ids::Visibility,
) -> nia_ids::ModuleId {
    let child = sym(child_name);
    graph
        .intern_declared_child(parent, &child, visibility, Span::default())
        .expect("intern child module")
}

fn defs_from_source(module_id: nia_ids::ModuleId, source: &str) -> DefCollection {
    let lowered = nia_parser::parse_versioned(
        source,
        Some(SourceVersion {
            id: SourceId::isolated(),
            revision: SourceRevision::INITIAL,
        }),
        nia_symbol_table::SymbolTable::new(),
    );
    let (module, parse_errors, _) = (lowered.module, lowered.errors, lowered.origins);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    nia_defs::collect_module_defs(module_id, &module).expect("collect definitions")
}

#[test]
fn type_equivalence_resolves_nominal_const_expression_summaries() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let left_module = module_ids.allocate().expect("allocate module ID");
    let right_module = module_ids.allocate().expect("allocate module ID");
    let type_store = TypeStore::new().expect("create type store");
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_usize = left.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let right_usize = right.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let def_id = GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(800),
    };
    let left_expr = nia_ids::GlobalConstExprId {
        module_id: left_module,
        const_expr_id: nia_ids::ConstExprId::synthetic(1),
    };
    let right_expr = nia_ids::GlobalConstExprId {
        module_id: right_module,
        const_expr_id: nia_ids::ConstExprId::synthetic(2),
    };
    let left_ty = left.test_intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: left_usize,
            value: nia_ty::ConstGenericValue::ConstExpr(left_expr.clone()),
        }],
    });
    let right_ty = right.test_intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::ConstExpr(right_expr.clone()),
        }],
    });
    let mut const_expr_summaries = HashMap::new();
    const_expr_summaries.insert(
        left_expr.clone(),
        nia_ty::ConstExprSummary {
            span: Span::default(),
            literal_array_len: Some(4),
        },
    );
    const_expr_summaries.insert(
        right_expr.clone(),
        nia_ty::ConstExprSummary {
            span: Span::default(),
            literal_array_len: Some(4),
        },
    );
    let lowering = nia_type_lower::TypeLowering {
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        const_expr_summaries,
        diagnostics: Vec::new(),
    };
    assert!(types_equivalent(&type_store, &lowering, left_ty, right_ty));
    let right_int = right.test_intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
        }],
    });
    assert!(types_equivalent(&type_store, &lowering, left_ty, right_int));
    let unresolved_expr = nia_ids::GlobalConstExprId {
        module_id: right_module,
        const_expr_id: nia_ids::ConstExprId::synthetic(3),
    };
    let unresolved = right.test_intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::ConstExpr(unresolved_expr),
        }],
    });
    assert!(!types_equivalent(
        &type_store,
        &lowering,
        left_ty,
        unresolved
    ));
}

#[test]
fn trait_goal_guard_resolves_const_expression_summaries() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let left_module = module_ids.allocate().expect("allocate module ID");
    let right_module = module_ids.allocate().expect("allocate module ID");
    let type_store = TypeStore::new().expect("create type store");
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_usize = left.test_primitive(PrimitiveTy::Usize);
    let right_usize = right.test_primitive(PrimitiveTy::Usize);
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(801),
    });
    let left_expr = nia_ids::GlobalConstExprId {
        module_id: left_module,
        const_expr_id: nia_ids::ConstExprId::synthetic(4),
    };
    let right_expr = nia_ids::GlobalConstExprId {
        module_id: right_module,
        const_expr_id: nia_ids::ConstExprId::synthetic(5),
    };
    let argument = |ty, expr| nia_ty::ConstGenericArg {
        ty,
        value: nia_ty::ConstGenericValue::ConstExpr(expr),
    };
    let left_goal = TraitGoal {
        self_ty: left_usize,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: vec![argument(left_usize, left_expr.clone())],
    };
    let right_goal = TraitGoal {
        self_ty: right_usize,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: vec![argument(right_usize, right_expr.clone())],
    };
    let mut const_expr_summaries = HashMap::new();
    for expr in [left_expr.clone(), right_expr.clone()] {
        const_expr_summaries.insert(
            expr,
            nia_ty::ConstExprSummary {
                span: Span::default(),
                literal_array_len: Some(6),
            },
        );
    }
    let lowering = nia_type_lower::TypeLowering {
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        const_expr_summaries,
        diagnostics: Vec::new(),
    };

    assert!(trait_goals_equivalent(
        &type_store,
        &lowering,
        &left_goal,
        &right_goal,
    ));
    let unresolved_goal = TraitGoal {
        trait_const_args: vec![argument(
            right_usize,
            nia_ids::GlobalConstExprId {
                module_id: right_module,
                const_expr_id: nia_ids::ConstExprId::synthetic(6),
            },
        )],
        ..right_goal
    };
    assert!(!trait_goals_equivalent(
        &type_store,
        &lowering,
        &left_goal,
        &unresolved_goal,
    ));
}

#[test]
fn visible_extension_provider_modules_follow_only_explicit_edges() {
    let mut graph =
        ModuleGraph::with_symbol_text(SourcePath::new("main.nia"), Arc::new(test_symbols()))
            .expect("create module graph");
    let entry = graph.entry();
    let facade = intern_child(&mut graph, entry, "facade", nia_ids::Visibility::Public);
    let reexported = intern_child(&mut graph, entry, "reexported", nia_ids::Visibility::Public);
    let nested = intern_child(&mut graph, entry, "nested", nia_ids::Visibility::Public);
    let private_import = intern_child(
        &mut graph,
        entry,
        "private_import",
        nia_ids::Visibility::Public,
    );
    let facade_provider = intern_child(
        &mut graph,
        entry,
        "facade_provider",
        nia_ids::Visibility::Public,
    );
    let direct_provider = intern_child(
        &mut graph,
        entry,
        "direct_provider",
        nia_ids::Visibility::Public,
    );

    graph.add_provider_dependency(entry, direct_provider);
    graph.add_provider_dependency(facade, facade_provider);
    let mut using_scope = ModuleUsingScope::default();
    using_scope.modules.insert(sym("facade"), facade);

    let mut facade_surface = nia_defs::ModulePublicSurface::new(facade);
    facade_surface
        .modules
        .insert(sym("implementation"), reexported);
    facade_surface
        .reexported_modules
        .insert(sym("implementation"), reexported);
    facade_surface
        .reexported_provider_modules
        .push(facade_provider);
    let mut reexported_surface = nia_defs::ModulePublicSurface::new(reexported);
    reexported_surface
        .reexported_modules
        .insert(sym("nested"), nested);
    reexported_surface.modules.insert(sym("nested"), nested);
    // A public child namespace is not itself a public-using re-export.
    reexported_surface
        .modules
        .insert(sym("private_import"), private_import);
    let surfaces = HashMap::from([
        (facade, Arc::new(facade_surface)),
        (reexported, Arc::new(reexported_surface)),
    ]);
    let public_surfaces = |module_id| surfaces.get(&module_id).cloned();

    let modules = visible_extension_provider_modules(VisibleExtensionProviderModulesInput {
        module_id: entry,
        graph: &graph,
        using_scope: &using_scope,
        public_surfaces: &public_surfaces,
    });

    let mut expected = vec![facade, reexported, nested, facade_provider, direct_provider];
    expected.sort();
    assert_eq!(modules, expected);
    assert!(!modules.contains(&private_import));
}

#[test]
fn direct_import_activates_the_imported_package_facade_providers() {
    let mut graph =
        ModuleGraph::with_symbol_text(SourcePath::new("main.nia"), Arc::new(test_symbols()))
            .expect("create module graph");
    let entry = graph.entry();
    let facade = intern_child(&mut graph, entry, "facade", nia_ids::Visibility::Public);
    let provider = intern_child(&mut graph, entry, "provider", nia_ids::Visibility::Public);

    let mut using_scope = ModuleUsingScope::default();
    using_scope.modules.insert(sym("facade"), facade);

    let mut root_surface = nia_defs::ModulePublicSurface::new(entry);
    root_surface.reexported_provider_modules.push(provider);
    let surfaces = HashMap::from([(entry, Arc::new(root_surface))]);
    let public_surfaces = |module_id| surfaces.get(&module_id).cloned();

    let modules = visible_extension_provider_modules(VisibleExtensionProviderModulesInput {
        module_id: entry,
        graph: &graph,
        using_scope: &using_scope,
        public_surfaces: &public_surfaces,
    });

    assert_eq!(modules, vec![facade, provider]);
}

#[test]
fn package_facade_providers_do_not_activate_through_a_reexported_child() {
    let mut graph =
        ModuleGraph::with_symbol_text(SourcePath::new("main.nia"), Arc::new(test_symbols()))
            .expect("create module graph");
    let entry = graph.entry();
    let facade = intern_child(&mut graph, entry, "facade", nia_ids::Visibility::Public);
    let dep_root = graph
        .intern_package_root(&sym("dep"), SourcePath::new("dep/root.nia"))
        .expect("intern dependency package root");
    let dep_facade = intern_child(&mut graph, dep_root, "facade", nia_ids::Visibility::Public);
    let dep_provider = intern_child(
        &mut graph,
        dep_root,
        "provider",
        nia_ids::Visibility::Public,
    );

    let mut using_scope = ModuleUsingScope::default();
    using_scope.modules.insert(sym("facade"), facade);

    let mut facade_surface = nia_defs::ModulePublicSurface::new(facade);
    facade_surface
        .reexported_modules
        .insert(sym("dep_facade"), dep_facade);
    let mut dep_root_surface = nia_defs::ModulePublicSurface::new(dep_root);
    dep_root_surface
        .reexported_provider_modules
        .push(dep_provider);
    let surfaces = HashMap::from([
        (facade, Arc::new(facade_surface)),
        (dep_root, Arc::new(dep_root_surface)),
    ]);
    let public_surfaces = |module_id| surfaces.get(&module_id).cloned();

    let modules = visible_extension_provider_modules(VisibleExtensionProviderModulesInput {
        module_id: entry,
        graph: &graph,
        using_scope: &using_scope,
        public_surfaces: &public_surfaces,
    });

    assert!(modules.contains(&facade));
    assert!(modules.contains(&dep_facade));
    assert!(
        !modules.contains(&dep_provider),
        "a dependency's package facade must be explicitly re-exported by the importing facade"
    );
}

#[test]
fn const_generic_supertrait_instances_require_exact_impl_arguments() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let type_store = TypeStore::new().expect("create type store");
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let target_ty = append.test_intern(TyKind::Primitive(PrimitiveTy::U8));
    let const_name = sym("N");
    let base_id = GlobalDefId {
        module_id,
        def_id: nia_defs::DefId(1),
    };
    let symbolic_supertrait = append.test_intern(TyKind::Nominal {
        def_id: base_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: usize_ty,
            value: nia_ty::ConstGenericValue::GenericParam(const_name),
        }],
    });
    let four = nia_ty::ConstGenericArg {
        ty: usize_ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
    };
    let eight = nia_ty::ConstGenericArg {
        ty: usize_ty,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(8)),
    };
    let params = [nia_item_signatures::GenericParamSignature {
        name: const_name,
        kind: nia_item_signatures::GenericParamSignatureKind::Const { ty: usize_ty },
    }];

    assert!(substitutions_from_generic_params(&params, &[], std::slice::from_ref(&four)).is_some());
    assert!(substitutions_from_generic_params(&params, &[], &[]).is_none());
    assert!(
        substitutions_from_generic_params(&params, &[], &[four.clone(), eight.clone()]).is_none()
    );

    let substituted = substitute_trait_bound(
        &append,
        &type_store,
        symbolic_supertrait,
        &params,
        &[],
        std::slice::from_ref(&four),
    )
    .expect("substitute trait bound")
    .expect("complete trait instance arguments");
    assert!(matches!(
        type_store.get(substituted),
        Some(TyKind::Nominal { const_args, .. }) if const_args == std::slice::from_ref(&four)
    ));

    let mut base_impl = ProgramTraitImplSignature {
        module_id,
        impl_id: TraitImplId(0),
        builtin: None,
        generics: Vec::new(),
        generic_params: Vec::new(),
        target_ty,
        trait_id: TraitId::Source(base_id),
        trait_args: Vec::new(),
        trait_const_args: vec![eight],
        where_predicates: Vec::new(),
        associated_types: Vec::new(),
        associated_values: Vec::new(),
    };
    assert!(!has_matching_trait_impl(
        &type_store,
        target_ty,
        TraitId::Source(base_id),
        &[],
        std::slice::from_ref(&four),
        std::slice::from_ref(&base_impl),
    ));
    base_impl.trait_const_args = vec![four.clone()];
    assert!(has_matching_trait_impl(
        &type_store,
        target_ty,
        TraitId::Source(base_id),
        &[],
        std::slice::from_ref(&four),
        std::slice::from_ref(&base_impl),
    ));
}

#[test]
fn projection_context_matching_uses_semantic_arguments() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let left_module = module_ids.allocate().expect("allocate module ID");
    let right_module = module_ids.allocate().expect("allocate module ID");
    let type_store = TypeStore::new().expect("create type store");
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_u32 = left.test_intern(TyKind::Primitive(PrimitiveTy::U32));
    let right_u32 = right.test_intern(TyKind::Primitive(PrimitiveTy::U32));
    let left_const = nia_ty::ConstGenericArg {
        ty: left_u32,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(7)),
    };
    let right_const = nia_ty::ConstGenericArg {
        ty: right_u32,
        value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(7)),
    };

    assert!(types::projection_context_matches(
        &type_store,
        left_u32,
        right_u32,
        &[left_u32],
        &[right_u32],
        &[left_const],
        &[right_const],
    ));
}

#[test]
fn substitutes_generic_type_inside_array_layout_builtin() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let defs = defs_from_source(module_id, "struct Dummy {}");
    let type_store = TypeStore::new().expect("create type store");
    let append = type_store.append_for_module(module_id);
    let generic = sym("T");
    let generic_ty = append.test_intern(TyKind::GenericParam(generic));
    let u32_ty = append.test_intern(TyKind::Primitive(PrimitiveTy::U32));
    let array = append.test_intern(TyKind::Array {
        len: nia_ty::ArrayLenTy::Builtin {
            builtin: nia_ty::LayoutBuiltin::Size,
            ty: generic_ty,
        },
        elem: u32_ty,
    });
    let empty_signatures = nia_item_signatures::ItemSignatures {
        functions: HashMap::new(),
        structs: HashMap::new(),
        unions: HashMap::new(),
        traits: HashMap::new(),
        trait_impls: Vec::new(),
        enums: HashMap::new(),
        type_aliases: HashMap::new(),
        globals: HashMap::new(),
        consts: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let lowering = nia_type_lower::TypeLowering {
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let normalization = nia_type_normalize::TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
        aliases: Default::default(),
    };
    let module = ExtensionModuleInput {
        module_id,
        type_store: &type_store,
        defs: &defs,
        lowering: &lowering,
        signatures: &empty_signatures,
        function_signatures: &empty_signatures,
        type_signatures: &empty_signatures,
        normalization: &normalization,
    };
    let substitutions = SymbolMap::from_iter([(generic, u32_ty)]);
    let substituted = substitute_type(
        &append,
        &module,
        &type_store,
        array,
        &substitutions,
        &SymbolMap::default(),
        TypeSubstitutionTarget::default(),
    )
    .expect("substitute array type");
    assert!(matches!(
        type_store.get(substituted),
        Some(TyKind::Array {
            len: nia_ty::ArrayLenTy::Builtin { ty, .. },
            ..
        }) if *ty == u32_ty
    ));
}

#[test]
fn trait_goal_assumption_identity_is_semantic_and_includes_self_type() {
    let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
    let left_module = module_ids.allocate().expect("allocate module ID");
    let right_module = module_ids.allocate().expect("allocate module ID");
    let type_store = TypeStore::new().expect("create type store");
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let usize_left = left.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let usize_right = right.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let target = GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(70),
    };
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(71),
    });
    let signed = left.test_intern(TyKind::Nominal {
        def_id: target,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: usize_left,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(9)),
        }],
    });
    let unsigned = right.test_intern(TyKind::Nominal {
        def_id: target,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: usize_right,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(9)),
        }],
    });
    let left_goal = TraitGoal {
        self_ty: signed,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
    };
    let right_goal = TraitGoal {
        self_ty: unsigned,
        ..left_goal.clone()
    };
    let lowering = nia_type_lower::TypeLowering {
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    };

    assert!(trait_goals_equivalent(
        &type_store,
        &lowering,
        &left_goal,
        &right_goal,
    ));
    let other = left.test_intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id: left_module,
            def_id: nia_defs::DefId(72),
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    assert!(!trait_goals_equivalent(
        &type_store,
        &lowering,
        &left_goal,
        &TraitGoal {
            self_ty: other,
            ..right_goal
        }
    ));
}
