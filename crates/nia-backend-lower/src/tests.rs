// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{BodyCheckInput, check_module_bodies_with_program_signatures_and_layouts};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlockId, FunctionExpr, FunctionExprKind, FunctionOp,
    FunctionTerminator,
};
use nia_function_lower::{FunctionTypeContext, lower_function_body};
use nia_ids::{DefId, GlobalDefId, LocalId, ModuleIdAllocator};
use nia_item_signatures::{
    ItemSignatureInput, ItemSignatureSource, ProgramFunctionSignature, collect_item_signatures,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
use nia_parser::parse_module_with_symbols;
use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_symbol_table::SymbolTable;
use nia_type_lower::{
    ProgramDefsContext, TypeLowering, TypeLoweringContext, lower_module_types_with_context,
};
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;
use std::sync::Arc;

struct TestBackendLowering {
    lowering: BackendLowering,
    module_id: ModuleId,
    type_store: nia_ty::TypeStore,
}

fn backend_function_instance_plan(
    monomorphization: &nia_monomorphize::Monomorphization,
) -> Vec<BackendFunctionInstancePlan> {
    monomorphization
        .instances
        .iter()
        .map(|instance| BackendFunctionInstancePlan {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            self_arg: instance.self_arg,
            args: instance.args.clone(),
            const_args: instance.const_args.clone(),
            span: instance.span,
        })
        .collect()
}

impl std::ops::Deref for TestBackendLowering {
    type Target = BackendLowering;

    fn deref(&self) -> &Self::Target {
        &self.lowering
    }
}

impl TestBackendLowering {
    fn append(&self, module_id: ModuleId) -> nia_ty::TypeStoreAppend {
        self.type_store.append_for_module(module_id)
    }
}

fn local_name(text: &str) -> nia_function_ir::LocalName {
    nia_function_ir::LocalName::named(sym(text))
}

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(nia_symbol::stable_hash(text))
}

#[test]
fn module_finalization_task_contract_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send::<BackendModuleFinalization>();
    assert_send_sync::<BackendProgramFinalizationContext<&'static nia_ty::TypeStore>>();
    assert_send_sync::<BackendProgramFinalizationContext>();
    assert_send_sync::<BackendLowerModuleInput<'static>>();
}

#[test]
fn module_finalizations_merge_in_program_order() {
    fn empty_module(id: ModuleId, name: &str) -> BackendModule {
        BackendModule {
            id,
            source_identity: nia_source::SourceIdentity::new(name),
            name: name.to_string(),
            const_eval: nia_backend_ir::BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: nia_layout::TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    fn change(module_id: ModuleId) -> BackendOptimizationChange {
        BackendOptimizationChange::Global {
            module_id,
            global: GlobalDefId {
                module_id,
                def_id: DefId(0),
            },
            pass: "test-pass",
        }
    }

    let mut module_ids = ModuleIdAllocator::new();
    let first = module_ids.allocate();
    let second = module_ids.allocate();
    let finalization = BackendItemPlanFinalization {
        optimization: OptimizationPolicy::default(),
        optimization_report: BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
        owner_directory: Arc::new(nia_backend_ir::BackendModuleOwnerDirectory::default()),
    };
    let completed_in_reverse_order = [
        BackendModuleFinalization {
            position: 1,
            module: empty_module(second, "second"),
            optimization_report: BackendOptimizationReport {
                changed_passes: vec![change(second)],
                ..BackendOptimizationReport::default()
            },
            diagnostics: vec![
                Diagnostic::internal_error(nia_diagnostic::codes::ICE, "second diagnostic")
                    .finish(),
            ],
        },
        BackendModuleFinalization {
            position: 0,
            module: empty_module(first, "first"),
            optimization_report: BackendOptimizationReport {
                changed_passes: vec![change(first)],
                ..BackendOptimizationReport::default()
            },
            diagnostics: vec![
                Diagnostic::internal_error(nia_diagnostic::codes::ICE, "first diagnostic").finish(),
            ],
        },
    ];

    let mut collector = BackendModuleFinalizationCollector::new(finalization, &[first, second]);
    let module_store = collector.module_store();
    let mut readiness = collector.take_readiness();
    for module_finalization in completed_in_reverse_order {
        let position = module_finalization.position;
        collector.push(position, module_finalization);
    }
    let second_ready = readiness.wait_next().expect("second completion");
    assert_eq!(second_ready.position(), 1);
    assert_eq!(second_ready.module_id(), second);
    let first_ready = readiness.wait_next().expect("first completion");
    assert_eq!(first_ready.position(), 0);
    assert_eq!(first_ready.module_id(), first);
    assert_eq!(readiness.wait_next(), None);
    assert_eq!(
        module_store
            .get(second)
            .expect("published second module")
            .id,
        second
    );
    let first_ptr = module_store.get(first).expect("published first module")
        as *const nia_backend_ir::BackendModule;
    let lowering = collector.finish();

    assert_eq!(
        &lowering.program.modules[0] as *const nia_backend_ir::BackendModule,
        first_ptr
    );
    assert_eq!(readiness.wait_next(), None);
    assert_eq!(
        lowering
            .program
            .modules
            .iter()
            .map(|module| module.id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(
        lowering.optimization_report.changed_passes,
        vec![change(first), change(second)]
    );
    assert_eq!(
        lowering
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["first diagnostic", "second diagnostic"]
    );
}

#[test]
fn foreign_backend_item_plan_groups_and_orders_source_functions_by_owner() {
    let mut module_ids = ModuleIdAllocator::new();
    let entry = module_ids.allocate();
    let child = module_ids.allocate();
    let entry_low = GlobalDefId {
        module_id: entry,
        def_id: DefId(1),
    };
    let entry_high = GlobalDefId {
        module_id: entry,
        def_id: DefId(3),
    };
    let child_function = GlobalDefId {
        module_id: child,
        def_id: DefId(2),
    };
    let mut pending = PendingForeignBackendItems::default();
    pending
        .functions
        .extend([entry_high, child_function, entry_low, entry_high]);
    let module_indices = HashMap::from([(child, 0), (entry, 1)]);

    let plan = pending.drain_plan(&module_indices, 2);

    assert_eq!(plan.functions_by_owner[0], vec![child_function]);
    assert_eq!(plan.functions_by_owner[1], vec![entry_low, entry_high]);
    assert!(pending.is_empty());
}

#[test]
#[should_panic(expected = "foreign backend source function owner")]
fn foreign_backend_item_plan_rejects_missing_owner_module() {
    let mut module_ids = ModuleIdAllocator::new();
    let missing = module_ids.allocate();
    let mut pending = PendingForeignBackendItems::default();
    pending.functions.push_back(GlobalDefId {
        module_id: missing,
        def_id: DefId(1),
    });

    let _ = pending.drain_plan(&HashMap::new(), 0);
}

struct EmptyBodyProgramSignatures {
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    globals: HashMap<GlobalDefId, nia_item_signatures::ProgramGlobalSignature>,
    consts: HashMap<GlobalDefId, nia_item_signatures::ProgramConstSignature>,
    structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

impl EmptyBodyProgramSignatures {
    fn new() -> Self {
        Self {
            functions: HashMap::new(),
            globals: HashMap::new(),
            consts: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            traits: HashMap::new(),
            type_aliases: HashMap::new(),
            trait_impls: Vec::new(),
        }
    }

    fn context(&self) -> ProgramSignatureContext<'_> {
        ProgramSignatureContext {
            lookup: self,
            trait_impls: &self.trait_impls,
            trait_impl_index: None,
        }
    }
}

impl ProgramSignatureLookup for EmptyBodyProgramSignatures {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        self.functions.get(&def_id).cloned()
    }

    fn global(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    fn const_eval(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramConstSignature> {
        self.consts.get(&def_id).cloned()
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    fn union(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    fn type_alias(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    fn trait_ids_with_method_named(&self, name: &nia_symbol::SymbolId) -> Vec<GlobalDefId> {
        self.traits
            .iter()
            .filter_map(|(trait_id, signature)| {
                signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| &method.name == name)
                    .then_some(*trait_id)
            })
            .collect()
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, nia_item_signatures::ProgramTraitSignature)> {
        self.traits.iter().find_map(|(trait_id, signature)| {
            signature
                .signature
                .methods
                .iter()
                .any(|method| {
                    GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    } == method_id
                })
                .then(|| (*trait_id, signature.clone()))
        })
    }
}

fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &TypeLowering,
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
    builder.extend_node_type_uses(
        type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree),
    );
    builder.finish()
}

struct TestBackendProgramFacts<'a> {
    const_array_lengths: HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
    function_body_ids: Vec<GlobalDefId>,
    function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
    static_init_ids: Vec<GlobalDefId>,
    static_inits: HashMap<GlobalDefId, &'a nia_static_ir::StaticInit>,
    extension_methods: nia_defs::ExtensionMethods,
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
}

impl<'a> TestBackendProgramFacts<'a> {
    fn new(
        const_array_lengths: HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
        function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
        static_inits: HashMap<GlobalDefId, &'a nia_static_ir::StaticInit>,
    ) -> Self {
        let mut function_body_ids = function_bodies.keys().copied().collect::<Vec<_>>();
        function_body_ids.sort_unstable();
        let mut static_init_ids = static_inits.keys().copied().collect::<Vec<_>>();
        static_init_ids.sort_unstable();
        Self {
            const_array_lengths,
            function_body_ids,
            function_bodies,
            static_init_ids,
            static_inits,
            extension_methods: nia_defs::ExtensionMethods::default(),
            functions: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            traits: HashMap::new(),
            type_aliases: HashMap::new(),
            trait_impls: Vec::new(),
            trait_impl_index: nia_item_signatures::ProgramTraitImplIndex::default(),
        }
    }
}

impl BackendProgramFacts for TestBackendProgramFacts<'_> {
    fn const_array_lengths(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_const_check::ConstArrayLengths> {
        self.const_array_lengths.get(&module_id).copied()
    }

    fn function_body_ids(&self) -> &[GlobalDefId] {
        &self.function_body_ids
    }

    fn function_body(&self, def_id: GlobalDefId) -> Option<&nia_function_ir::FunctionBody> {
        self.function_bodies.get(&def_id).copied()
    }

    fn static_init_ids(&self) -> &[GlobalDefId] {
        &self.static_init_ids
    }

    fn static_init(&self, def_id: GlobalDefId) -> Option<&nia_static_ir::StaticInit> {
        self.static_inits.get(&def_id).copied()
    }

    fn extension_methods(&self) -> &nia_defs::ExtensionMethods {
        &self.extension_methods
    }

    fn extensions(&self, _module_id: ModuleId) -> Option<&VisibleExtensionMethods> {
        None
    }

    fn defs(&self, _module_id: ModuleId) -> Option<&DefCollection> {
        None
    }

    fn normalized_type(&self, _ty: InternedTyId) -> Option<InternedTyId> {
        None
    }

    fn normalized_type_from_module(
        &self,
        _module_id: ModuleId,
        _ty: InternedTyId,
    ) -> Option<InternedTyId> {
        None
    }

    fn functions(&self) -> &HashMap<GlobalDefId, ProgramFunctionSignature> {
        &self.functions
    }

    fn structs(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature> {
        &self.structs
    }

    fn unions(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature> {
        &self.unions
    }

    fn enums(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature> {
        &self.enums
    }

    fn traits(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature> {
        &self.traits
    }

    fn type_aliases(
        &self,
    ) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature> {
        &self.type_aliases
    }

    fn trait_impls(&self) -> &[nia_item_signatures::ProgramTraitImplSignature] {
        &self.trait_impls
    }

    fn trait_impl_index(&self) -> &nia_item_signatures::ProgramTraitImplIndex {
        &self.trait_impl_index
    }
}

mod cfg_and_scalar_passes;
mod diagnostics;
mod inlining_and_cross_function;
mod local_optimizations;
mod lowering;
mod reachability_and_instances;
mod static_initializers;

fn lower_source(source: &str) -> TestBackendLowering {
    let lowering = lower_source_with_const_mutation(source, |_, _| {});
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}

fn lower_source_with_const_mutation(
    source: &str,
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
) -> TestBackendLowering {
    lower_source_with_body_mutation_const_mutation_and_optimization(
        source,
        |_| {},
        mutate_const,
        nia_opt::OptimizationPolicy::default(),
    )
}

fn lower_source_with_body_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _, _| {},
        |_, _| {},
        optimization,
    )
}

fn lower_source_with_body_mutation_const_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _, _| {},
        mutate_const,
        optimization,
    )
}

fn lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &nia_ty::TypeStore,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_check_mutation_and_optimization(
        source,
        mutate_body,
        mutate_extensions,
        mutate_const,
        |_, _, _, _, _| {},
        optimization,
    )
}

fn lower_source_with_body_check_mutation_and_optimization(
    source: &str,
    mut mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &nia_ty::TypeStore,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    mutate_body_check: impl FnOnce(
        &mut nia_body_check::BodyCheck,
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &ItemSignatures,
        &nia_ty::TypeStoreAppend,
    ),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let type_store = nia_ty::TypeStore::new();
    let program_defs =
        |candidate| (candidate == module_id).then(|| std::sync::Arc::new(defs.clone()));
    let type_lowering = lower_module_types_with_context(
        module_id,
        &module,
        &type_resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &type_lowering,
        type_store: &type_store,
        symbols: None,
    });
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses = semantic_use_table(
        module_id,
        &values,
        &locals,
        &type_lowering,
        &active_item_tree,
    );
    let normalization_input = type_lowering.explicit_type_roots();
    let normalization = normalize_module_types(nia_type_normalize::TypeNormalizationInput {
        module_id,
        type_store: &type_store,
        input_ids: &normalization_input,
        signatures: &signatures,
    });
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-backend-lower-test/main.nia");
    let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &type_lowering.const_exprs,
        source_path: &source_path,
    });
    assert!(
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let const_input = nia_const_check::ConstInput {
        type_store: &type_store,
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        source_path: &source_path,
        program: nia_const_check::ConstProgramContext::empty(),
    };
    let const_eval = nia_const_check::check_module_const(const_input);
    let root_types = signatures.type_roots();
    let layouts =
        nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: &type_store,
            defs: &defs,
            signatures: &signatures,
            root_types: &root_types,
            normalized: &normalization.normalized,
            array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
            target: nia_layout::TargetDataLayout::LP64,
            program: nia_layout::ProgramLayoutContext::default(),
        });
    let const_array_lengths = nia_const_check::ConstArrayLengths {
        values: const_eval.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let const_values = nia_const_check::ConstValues {
        values: const_eval.values.clone(),
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let const_typed_facts = nia_const_check::ConstTypedFacts {
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let body_const = nia_body_check::BodyConst::from_phases(
        &const_values,
        &const_array_lengths,
        &const_typed_facts,
    );
    let mut extensions = VisibleExtensionMethods::default();
    mutate_extensions(
        &mut extensions,
        &defs,
        &type_store,
        &type_lowering,
        &signatures,
    );
    let origins = NodeOriginTable::default();
    let program_signatures = EmptyBodyProgramSignatures::new();
    let mut body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        type_store: &type_store,
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
        const_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        const_eval: body_const,
        const_module: &const_module.module,
        layouts: &layouts,
        extensions: &extensions,
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: nia_body_check::FunctionCheckScope::LocalModule,
        program_const: nia_body_check::ProgramConstMaps::empty(),
        filter: nia_body_check::BodyCheckFilter::All,
        product: nia_body_check::BodyCheckProduct::Full,
        prechecked: None,
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    let body_types = type_store.append_for_module(module_id);
    mutate_body_check(&mut body_check, &module, &defs, &signatures, &body_types);
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| {
            let mut body = lower_function_body(
                module_id,
                body,
                FunctionTypeContext::for_module(&type_store, module_id),
            )
            .expect("valid typed body")
            .body;
            mutate_body(&mut body);
            (*def_id, body)
        })
        .collect::<HashMap<_, _>>();
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::default();
    let semantic_instantiations = body_check
        .facts
        .iter_generic_instantiations()
        .cloned()
        .collect::<Vec<_>>();
    let monomorphization = nia_monomorphize::collect_monomorphizations(
        &[nia_monomorphize::MonomorphizeModuleInput {
            module_id,
            defs: &defs,
            normalization: &normalization,
            const_eval: &const_eval,
            const_expr_summaries: &type_lowering.const_expr_summaries,
            layouts: Some(&layouts),
            local_enums: &signatures.enums,
            program_enums: &HashMap::new(),
            trait_impls: &[],
            trait_impl_index: &trait_impl_index,
            instantiations: &semantic_instantiations,
        }],
        &type_store,
    );
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );
    let function_instance_plan = backend_function_instance_plan(&monomorphization);
    let mut const_eval = const_eval;
    mutate_const(&mut const_eval, &type_lowering);
    let const_array_lengths = nia_const_check::ConstArrayLengths {
        values: const_eval.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let program_const = HashMap::from([(module_id, &const_array_lengths)]);
    let const_enum_values = const_enum_values_from_check(&const_eval);
    let program_function_bodies = function_bodies
        .iter()
        .map(|(def_id, body)| (*def_id, body))
        .collect::<HashMap<_, _>>();
    let program_static_inits = body_check
        .ir
        .global_inits
        .iter()
        .map(|(def_id, init)| (*def_id, init.as_ref()))
        .collect::<HashMap<_, _>>();
    let program =
        TestBackendProgramFacts::new(program_const, program_function_bodies, program_static_inits);

    let input = BackendLowerModuleInput {
        module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        module_name: "main".to_string(),
        symbols: &symbols,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        semantic_facts: &body_check.facts,
        extensions: &extensions,
        const_array_lengths: &const_array_lengths,
        const_enum_values: &const_enum_values,
        layouts: &layouts,
        roots: BackendFunctionRoots::Public,
        reachable_functions: None,
        reachable_globals: None,
        reachable_structs: None,
        reachable_unions: None,
        function_instance_plan: &function_instance_plan,
        program: &program,
    };
    let lowering = lower_backend_program(&[input], &type_store, optimization);
    TestBackendLowering {
        lowering,
        module_id,
        type_store,
    }
}

fn const_enum_values_from_check(
    const_eval: &nia_const_check::ConstCheck,
) -> nia_const_check::ConstEnumValues {
    nia_const_check::ConstEnumValues {
        values: const_eval.enum_values.clone(),
        typed_values: const_eval.typed_enum_values.clone(),
        diagnostics: Vec::new(),
    }
}

fn active_item_tree(module: &nia_ast::Module) -> ActiveModuleItemTree {
    let item_tree = ModuleItemTree::from_module(module);
    ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default())
}

fn global_def_id_by_name(defs: &nia_defs::DefCollection, name: &str) -> GlobalDefId {
    let name_symbol = sym(name);
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == name_symbol).then_some(GlobalDefId {
                module_id: defs.module_id,
                def_id,
            })
        })
        .unwrap_or_else(|| panic!("missing def `{name}`"))
}

fn nominal_type_by_def(
    type_store: &nia_ty::TypeStore,
    lowering: &TypeLowering,
    target: GlobalDefId,
) -> InternedTyId {
    nominal_type_by_def_with_args(type_store, lowering, target, &[])
}

fn nominal_type_by_def_with_args(
    type_store: &nia_ty::TypeStore,
    lowering: &TypeLowering,
    target: GlobalDefId,
    target_args: &[InternedTyId],
) -> InternedTyId {
    lowering
        .explicit_type_roots()
        .into_iter()
        .find(|ty| {
            matches!(
                type_store.get(*ty),
                Some(nia_ty::TyKind::Nominal {
                    def_id,
                    args,
                    ..
                }) if *def_id == target && args == target_args
            )
        })
        .unwrap_or_else(|| panic!("missing nominal type {target:?} with args {target_args:?}"))
}

fn first_terminal_value(body: &nia_function_ir::FunctionBody) -> &nia_function_ir::FunctionExpr {
    body.blocks
        .iter()
        .find_map(|block| match &block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}

fn first_terminal_value_mut(
    body: &mut nia_function_ir::FunctionBody,
) -> &mut nia_function_ir::FunctionExpr {
    body.blocks
        .iter_mut()
        .find_map(|block| match &mut block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}
