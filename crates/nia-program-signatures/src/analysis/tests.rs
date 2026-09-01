// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::{ModuleUsingScope, PublicNamespace, UsingEntry};
use nia_imports::ModuleGraph;
use nia_item_signatures::ProgramTypeAliasSignature;
use nia_source::{SourceId, SourcePath, SourceRevision, SourceVersion};
use nia_span::Span;
use nia_symbol::stable_hash;
use std::{cell::RefCell, collections::HashMap, sync::Arc};

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
    let syntax = nia_syntax::parse_source(
        source,
        Some(SourceVersion {
            id: SourceId(module_id.local_index()),
            revision: SourceRevision::INITIAL,
        }),
    );
    let (module, parse_errors, _) = nia_parser::parse_module_syntax_with_origins(&syntax);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    nia_defs::collect_module_defs(module_id, &module)
}

fn global_type_def_id(defs: &DefCollection, name: &str) -> GlobalDefId {
    let def_id = defs
        .module_scope
        .types
        .get(&sym(name))
        .unwrap_or_else(|| panic!("missing type definition `{name}`"));
    GlobalDefId {
        module_id: defs.module_id,
        def_id,
    }
}

struct SingleModuleDefs {
    module_id: nia_ids::ModuleId,
    defs: Arc<DefCollection>,
}

impl ProgramDefsResolver for SingleModuleDefs {
    fn defs(&self, module_id: nia_ids::ModuleId) -> Option<Arc<DefCollection>> {
        (module_id == self.module_id).then(|| Arc::clone(&self.defs))
    }
}

fn using_type_entry(def_id: GlobalDefId) -> UsingEntry {
    UsingEntry {
        target_module: def_id.module_id,
        target_def_id: def_id.def_id,
        namespace: PublicNamespace::Type,
        directive_span: Span::default(),
        name_span: Span::default(),
        parent_enum: None,
    }
}

#[test]
fn type_equivalence_resolves_nominal_const_expression_summaries() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = TypeStore::new();
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_usize = left.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let right_usize = right.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let def_id = GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(800),
    };
    let left_expr = nia_ids::GlobalConstExprId {
        module_id: left_module,
        const_expr_id: nia_ids::ConstExprId(1),
    };
    let right_expr = nia_ids::GlobalConstExprId {
        module_id: right_module,
        const_expr_id: nia_ids::ConstExprId(2),
    };
    let left_ty = left.intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: left_usize,
            value: nia_ty::ConstGenericValue::ConstExpr(left_expr),
        }],
    });
    let right_ty = right.intern(TyKind::Nominal {
        def_id,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: right_usize,
            value: nia_ty::ConstGenericValue::ConstExpr(right_expr),
        }],
    });
    let mut const_expr_summaries = HashMap::new();
    const_expr_summaries.insert(
        left_expr,
        nia_ty::ConstExprSummary {
            span: Span::default(),
            literal_array_len: Some(4),
        },
    );
    const_expr_summaries.insert(
        right_expr,
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
    let right_int = right.intern(TyKind::Nominal {
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
        const_expr_id: nia_ids::ConstExprId(3),
    };
    let unresolved = right.intern(TyKind::Nominal {
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = TypeStore::new();
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_usize = left.primitive(PrimitiveTy::Usize);
    let right_usize = right.primitive(PrimitiveTy::Usize);
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(801),
    });
    let left_expr = nia_ids::GlobalConstExprId {
        module_id: left_module,
        const_expr_id: nia_ids::ConstExprId(4),
    };
    let right_expr = nia_ids::GlobalConstExprId {
        module_id: right_module,
        const_expr_id: nia_ids::ConstExprId(5),
    };
    let argument = |ty, expr| nia_ty::ConstGenericArg {
        ty,
        value: nia_ty::ConstGenericValue::ConstExpr(expr),
    };
    let left_goal = TraitGoal {
        self_ty: left_usize,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: vec![argument(left_usize, left_expr)],
    };
    let right_goal = TraitGoal {
        self_ty: right_usize,
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: vec![argument(right_usize, right_expr)],
    };
    let mut const_expr_summaries = HashMap::new();
    for expr in [left_expr, right_expr] {
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
                const_expr_id: nia_ids::ConstExprId(6),
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
fn visible_extension_provider_modules_batches_provider_targets_by_closure_wave() {
    let mut graph =
        ModuleGraph::with_symbol_text(SourcePath::new("main.nia"), Arc::new(test_symbols()));
    let entry = graph.entry();
    let types_module = intern_child(&mut graph, entry, "types", nia_ids::Visibility::Public);
    let used_provider = intern_child(
        &mut graph,
        entry,
        "used_provider",
        nia_ids::Visibility::Public,
    );
    let other_provider = intern_child(
        &mut graph,
        entry,
        "other_provider",
        nia_ids::Visibility::Public,
    );
    assert_eq!(
        [
            entry.local_index(),
            types_module.local_index(),
            used_provider.local_index(),
            other_provider.local_index(),
        ],
        [0, 1, 2, 3]
    );
    let type_defs = defs_from_source(types_module, "pub struct Other {} pub struct Used {}");
    let used = global_type_def_id(&type_defs, "Used");
    let other = global_type_def_id(&type_defs, "Other");
    let mut using_scope = ModuleUsingScope::default();
    using_scope
        .types
        .insert(sym("Used"), using_type_entry(used));
    using_scope
        .types
        .insert(sym("Other"), using_type_entry(other));
    let using_scopes = |_module_id| None::<Arc<ModuleUsingScope>>;
    let type_alias = |_def_id| None::<ProgramTypeAliasSignature>;
    let type_store = nia_ty::TypeStore::new();
    let empty_normalization = TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let defs = SingleModuleDefs {
        module_id: type_defs.module_id,
        defs: Arc::new(type_defs.clone()),
    };
    let normalizations = |_module_id| Some(Arc::new(empty_normalization.clone()));
    let calls = RefCell::new(Vec::<Vec<GlobalDefId>>::new());
    let nominal_extension_providers = |targets: &[GlobalDefId]| {
        calls.borrow_mut().push(targets.to_vec());
        targets
            .iter()
            .filter_map(|target| {
                if *target == used {
                    Some(used_provider)
                } else if *target == other {
                    Some(other_provider)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };

    let modules = visible_extension_provider_modules(VisibleExtensionProviderModulesInput {
        module_id: entry,
        type_store: &type_store,
        graph: &graph,
        using_scope: &using_scope,
        using_scopes: &using_scopes,
        defs: &defs,
        normalizations: &normalizations,
        visible_type_signatures: VisibleTypeSignatures {
            type_alias: &type_alias,
        },
        nominal_extension_providers: &nominal_extension_providers,
    });

    assert_eq!(modules, vec![used_provider, other_provider]);
    let calls = calls.borrow();
    assert_eq!(
        calls.len(),
        1,
        "provider targets in one visibility-closure wave should be batched"
    );
    assert_eq!(calls[0], vec![other, used]);
}

#[test]
fn const_generic_supertrait_instances_require_exact_impl_arguments() {
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let usize_ty = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let target_ty = append.intern(TyKind::Primitive(PrimitiveTy::U8));
    let const_name = sym("N");
    let base_id = GlobalDefId {
        module_id,
        def_id: nia_defs::DefId(1),
    };
    let symbolic_supertrait = append.intern(TyKind::Nominal {
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = TypeStore::new();
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let left_u32 = left.intern(TyKind::Primitive(PrimitiveTy::U32));
    let right_u32 = right.intern(TyKind::Primitive(PrimitiveTy::U32));
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let defs = defs_from_source(module_id, "struct Dummy {}");
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let generic = sym("T");
    let generic_ty = append.intern(TyKind::GenericParam(generic));
    let u32_ty = append.intern(TyKind::Primitive(PrimitiveTy::U32));
    let array = append.intern(TyKind::Array {
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
    );
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
    let mut module_ids = nia_ids::ModuleIdAllocator::new();
    let left_module = module_ids.allocate();
    let right_module = module_ids.allocate();
    let type_store = TypeStore::new();
    let left = type_store.append_for_module(left_module);
    let right = type_store.append_for_module(right_module);
    let usize_left = left.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let usize_right = right.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let target = GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(70),
    };
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: left_module,
        def_id: nia_defs::DefId(71),
    });
    let signed = left.intern(TyKind::Nominal {
        def_id: target,
        args: Vec::new(),
        const_args: vec![nia_ty::ConstGenericArg {
            ty: usize_left,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(9)),
        }],
    });
    let unsigned = right.intern(TyKind::Nominal {
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
    let other = left.intern(TyKind::Nominal {
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
