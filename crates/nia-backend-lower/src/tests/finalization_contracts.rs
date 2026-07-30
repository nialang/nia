// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
                enums: Vec::new(),
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
