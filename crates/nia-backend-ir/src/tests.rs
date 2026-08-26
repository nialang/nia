use nia_function_ir::{FunctionBlockId, FunctionBody, FunctionInstanceKey};
use nia_ids::{ClosureId, DefId, GlobalDefId, ModuleIdAllocator};
use nia_layout::TargetDataLayout;
use nia_symbol::SymbolId;
use nia_ty::PrimitiveTy;

use super::*;

fn module_with_global(
    module_id: ModuleId,
    ty: InternedTyId,
    name: &str,
    is_extern: bool,
) -> BackendModule {
    BackendModule {
        id: module_id,
        source_identity: SourceIdentity::new(name),
        name: name.to_string(),
        const_eval: BackendConstFacts::default(),
        layouts: BackendLayouts {
            target: TargetDataLayout::LP64,
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
        globals: vec![BackendGlobal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(0),
            },
            name: SymbolId::EMPTY,
            link_name: None,
            ty,
            is_let: false,
            is_extern,
            init: None,
            span: Span::default(),
        }],
        global_instances: Vec::new(),
        functions: Vec::new(),
        function_instances: Vec::new(),
        closure_entries: Vec::new(),
        trait_object_vtables: Vec::new(),
        generic_instantiations: Vec::new(),
    }
}

#[test]
fn closure_entry_keys_distinguish_source_and_concrete_instance_owners() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let owner = GlobalDefId {
        module_id,
        def_id: DefId(7),
    };
    let closure_id = ClosureId { owner, ordinal: 1 };
    let source = BackendClosureEntryKey {
        closure_id,
        owner: BackendClosureEntryOwner::Source(owner),
    };
    let instance = BackendClosureEntryKey {
        closure_id,
        owner: BackendClosureEntryOwner::FunctionInstance(FunctionInstanceKey {
            def_id: owner,
            arg_module_id: module_id,
            self_arg: None,
            args: Vec::new(),
            const_args: Vec::new(),
        }),
    };

    assert_ne!(source, instance);
    assert_eq!(HashSet::from([source, instance]).len(), 2);
}

#[test]
fn backend_layout_conversion_uses_the_layout_product_owner() {
    let mut module_ids = ModuleIdAllocator::new();
    let owner = module_ids.allocate();
    let def_id = DefId(7);
    let layout = nia_layout::StructLayout {
        layout: nia_layout::TypeLayout { size: 4, align: 4 },
        fields: Vec::new(),
    };
    let instance_key = nia_layout::StructLayoutKey {
        def_id,
        args: Vec::new(),
        const_args: Vec::new(),
    };
    let layouts = nia_layout::Layouts {
        module_id: owner,
        target: TargetDataLayout::LP64,
        types: Default::default(),
        structs: HashMap::from([(def_id, layout.clone())]),
        unions: Default::default(),
        enums: Default::default(),
        struct_instances: HashMap::from([(instance_key, layout)]),
        union_instances: Default::default(),
        diagnostics: Vec::new(),
    };

    let backend = BackendLayouts::from_module_layouts(&layouts);

    assert_eq!(backend.structs[0].0.module_id, owner);
    assert_eq!(backend.struct_instances[0].0.def_id.module_id, owner);
}

#[test]
fn partitioning_defers_dangling_closure_instance_owners_to_validation() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
    let owner = GlobalDefId {
        module_id,
        def_id: DefId(7),
    };
    let owner_instance = FunctionInstanceKey {
        def_id: owner,
        arg_module_id: module_id,
        self_arg: None,
        args: Vec::new(),
        const_args: Vec::new(),
    };
    let mut module = module_with_global(module_id, ty, "main", false);
    module.closure_entries = (0..SOURCE_CODEGEN_SPLIT_THRESHOLD - 1)
        .map(|ordinal| BackendClosureEntry {
            key: BackendClosureEntryKey {
                closure_id: ClosureId {
                    owner,
                    ordinal: ordinal as u32,
                },
                owner: BackendClosureEntryOwner::FunctionInstance(owner_instance.clone()),
            },
            symbol: format!("dangling-closure-{ordinal}"),
            abi: BackendClosureEntryAbi {
                state_type: ty,
                state_pointer_type: ty,
                params: Vec::new(),
                return_type: ty,
            },
            state_param: nia_ids::LocalId(0),
            params: Vec::new(),
            local_names: HashMap::new(),
            function_body: FunctionBody {
                span: Span::default(),
                locals: Vec::new(),
                scopes: Vec::new(),
                blocks: Vec::new(),
                entry: FunctionBlockId(0),
                ty,
            },
            span: Span::default(),
        })
        .collect();

    let plan = CodegenPartitionPlan::for_ready_module(&module);
    let partitioned_entries = plan
        .partitions()
        .iter()
        .map(|partition| partition.closure_entry_definitions().len())
        .sum::<usize>();

    assert_eq!(partitioned_entries, SOURCE_CODEGEN_SPLIT_THRESHOLD - 1);
}

#[test]
fn owner_directory_records_actual_instance_publication_module() {
    let mut module_ids = ModuleIdAllocator::new();
    let semantic_owner = module_ids.allocate();
    let publication_owner = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(semantic_owner)
        .primitive(PrimitiveTy::I32);
    let def_id = GlobalDefId {
        module_id: semantic_owner,
        def_id: DefId(7),
    };
    let mut module = module_with_global(publication_owner, ty, "publication", false);
    module.global_instances.push(BackendGlobalInstance {
        def_id,
        name: SymbolId::EMPTY,
        arg_module_id: publication_owner,
        args: vec![ty],
        const_args: Vec::new(),
        symbol: "instance".to_string(),
        ty,
        is_let: false,
        init: None,
        span: Span::default(),
    });
    let key = BackendGlobalInstanceKey {
        def_id,
        arg_module_id: publication_owner,
        args: vec![ty],
        const_args: Vec::new(),
    };

    let directory = BackendModuleOwnerDirectory::from_modules([&module]);

    assert_eq!(
        directory.global_instance_owner(&key),
        Some(publication_owner)
    );
    assert_ne!(directory.global_instance_owner(&key), Some(semantic_owner));
}

#[test]
fn codegen_partitions_are_definition_filtered_and_stable_key_ordered() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let declaration_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let first_ty = type_store
        .append_for_module(first_id)
        .primitive(PrimitiveTy::I32);
    let declaration_ty = type_store
        .append_for_module(declaration_id)
        .primitive(PrimitiveTy::I32);
    let second_ty = type_store
        .append_for_module(second_id)
        .primitive(PrimitiveTy::I32);
    let program = BackendProgram::new(vec![
        module_with_global(second_id, second_ty, "second", false),
        module_with_global(declaration_id, declaration_ty, "declaration", true),
        module_with_global(first_id, first_ty, "first", false),
    ]);

    let plan = program.codegen_partition_plan();
    let partitions = plan.partitions();
    assert_eq!(
        partitions
            .iter()
            .map(|partition| partition.id)
            .collect::<Vec<_>>(),
        vec![
            CodegenUnitId::SourceModule {
                module_id: first_id,
                ordinal: 0,
            },
            CodegenUnitId::SourceModule {
                module_id: second_id,
                ordinal: 0,
            },
        ]
    );
    assert_eq!(program.module_for_partition(&partitions[0]).name, "first");
    assert_eq!(program.module_for_partition(&partitions[1]).name, "second");
    let first_module = program
        .modules
        .iter()
        .find(|module| module.id == first_id)
        .expect("first ready module");
    assert_eq!(
        CodegenPartitionPlan::for_ready_module(first_module).partitions(),
        &partitions[..1]
    );
    for partition in partitions {
        assert_eq!(partition.global_definitions(), &[0]);
        assert!(partition.global_instance_definitions().is_empty());
        assert!(partition.function_definitions().is_empty());
        assert!(partition.function_instance_definitions().is_empty());
        assert!(partition.vtable_definitions().is_empty());
    }
    assert_eq!(
        partitions
            .iter()
            .map(|partition| partition.key.clone())
            .collect::<Vec<_>>(),
        vec![
            CodegenUnitKey::SourceModule {
                source_identity: SourceIdentity::new("first"),
                ordinal: 0,
            },
            CodegenUnitKey::SourceModule {
                source_identity: SourceIdentity::new("second"),
                ordinal: 0,
            },
        ]
    );
}

#[test]
fn codegen_partition_membership_canonicalizes_instance_order() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
    let mut module = module_with_global(module_id, ty, "main", false);
    let instance = |def_id, symbol: &str| BackendFunctionInstance {
        def_id: GlobalDefId { module_id, def_id },
        name: SymbolId::EMPTY,
        arg_module_id: module_id,
        self_arg: None,
        args: Vec::new(),
        const_args: Vec::new(),
        symbol: symbol.to_string(),
        params: Vec::new(),
        return_type: ty,
        is_extern: false,
        is_variadic: false,
        attributes: Vec::new(),
        local_names: HashMap::new(),
        function_body: Some(FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: Vec::new(),
            blocks: Vec::new(),
            entry: FunctionBlockId(0),
            ty,
        }),
        span: Span::default(),
    };
    module.function_instances = vec![
        instance(DefId(2), "z-instance"),
        instance(DefId(1), "a-instance"),
    ];

    let definitions = CodegenPartitionDefinitions::from_module(&module);

    assert_eq!(definitions.function_instances, vec![1, 0]);
}

#[test]
fn codegen_partition_order_does_not_depend_on_module_id_allocation() {
    let mut module_ids = ModuleIdAllocator::new();
    let z_id = module_ids.allocate();
    let a_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let z_ty = type_store
        .append_for_module(z_id)
        .primitive(PrimitiveTy::I32);
    let a_ty = type_store
        .append_for_module(a_id)
        .primitive(PrimitiveTy::I32);
    let program = BackendProgram::new(vec![
        module_with_global(z_id, z_ty, "z", false),
        module_with_global(a_id, a_ty, "a", false),
    ]);

    let plan = program.codegen_partition_plan();

    assert_eq!(program.module_for_partition(&plan.partitions()[0]).id, a_id);
    assert_eq!(program.module_for_partition(&plan.partitions()[1]).id, z_id);
}

#[test]
fn large_source_modules_use_stable_bounded_definition_buckets() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
    let mut module = module_with_global(module_id, ty, "main", false);
    let template = module.globals[0].clone();
    module.globals = (0..8)
        .map(|index| BackendGlobal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(index),
            },
            ..template.clone()
        })
        .collect();
    let program = BackendProgram::new(vec![module]);

    let plan = program.codegen_partition_plan();

    assert_eq!(plan.partitions().len(), SOURCE_CODEGEN_BUCKETS);
    for (ordinal, partition) in plan.partitions().iter().enumerate() {
        assert_eq!(
            partition.id,
            CodegenUnitId::SourceModule {
                module_id,
                ordinal: ordinal as u32,
            }
        );
        assert_eq!(partition.global_definitions(), &[ordinal, ordinal + 4]);
    }
}

#[test]
fn codegen_bucket_assignment_uses_full_stable_numeric_width() {
    assert_eq!(super::stable_numeric_bucket(u64::MAX), 3);
    assert_eq!(
        super::stable_numeric_bucket((u64::from(u32::MAX) << 32) | 1),
        1
    );
}

#[test]
#[should_panic(expected = "duplicate stable codegen partition key")]
fn codegen_partition_plan_rejects_duplicate_stable_source_keys() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let first_ty = type_store
        .append_for_module(first_id)
        .primitive(PrimitiveTy::I32);
    let second_ty = type_store
        .append_for_module(second_id)
        .primitive(PrimitiveTy::I32);
    let program = BackendProgram::new(vec![
        module_with_global(first_id, first_ty, "same", false),
        module_with_global(second_id, second_ty, "same", false),
    ]);

    let _ = program.codegen_partition_plan();
}

#[test]
#[should_panic(expected = "duplicate trait-object vtable definition")]
fn codegen_partition_plan_rejects_duplicate_vtable_definitions() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(first_id)
        .primitive(PrimitiveTy::I32);
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: first_id,
        def_id: DefId(1),
    });
    let vtable = BackendTraitObjectVtable {
        key: BackendTraitObjectVtableKey {
            self_ty: ty,
            object_ty: ty,
        },
        trait_id,
        trait_args: Vec::new(),
        trait_const_args: Vec::new(),
        entries: Vec::new(),
        span: Span::default(),
    };
    let mut first = module_with_global(first_id, ty, "first", false);
    first.trait_object_vtables.push(vtable.clone());
    let mut second = module_with_global(second_id, ty, "second", false);
    second.trait_object_vtables.push(vtable);
    let program = BackendProgram::new(vec![first, second]);

    let _ = program.codegen_partition_plan();
}

#[test]
#[should_panic(expected = "codegen partition plan does not match")]
fn codegen_partition_plan_rejects_definition_membership_mutation() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
    let program = BackendProgram::new(vec![module_with_global(module_id, ty, "main", false)]);
    let plan = program.codegen_partition_plan();
    let mut changed_module = module_with_global(module_id, ty, "main", false);
    changed_module.globals.clear();
    let program = BackendProgram::new(vec![changed_module]);

    plan.validate_program(&program);
}

#[test]
fn codegen_unit_dependencies_preserve_unit_and_canonicalize_modules() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let unit = CodegenUnitId::SourceModule {
        module_id: first_id,
        ordinal: 2,
    };

    let dependencies = CodegenUnitDependencies::new(unit, [second_id, first_id, second_id]);

    assert_eq!(dependencies.unit(), unit);
    assert_eq!(dependencies.modules(), &[first_id, second_id]);
    assert!(dependencies.contains(first_id));
    assert!(dependencies.contains(second_id));
}

#[test]
#[should_panic(expected = "dependency modules must include its owner")]
fn codegen_unit_dependencies_reject_empty_module_sets() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();

    let _ = CodegenUnitDependencies::new(
        CodegenUnitId::SourceModule {
            module_id,
            ordinal: 0,
        },
        [],
    );
}

#[test]
fn backend_module_store_publishes_concurrently_without_moving_payloads() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let first_ty = type_store
        .append_for_module(first_id)
        .primitive(PrimitiveTy::I32);
    let second_ty = type_store
        .append_for_module(second_id)
        .primitive(PrimitiveTy::I32);
    let store = std::sync::Arc::new(BackendModuleStore::new([first_id, second_id]));
    let first_store = std::sync::Arc::clone(&store);
    let second_store = std::sync::Arc::clone(&store);

    let second = std::thread::spawn(move || {
        second_store.publish(module_with_global(second_id, second_ty, "second", false));
    });
    let first = std::thread::spawn(move || {
        first_store.publish(module_with_global(first_id, first_ty, "first", false));
    });
    second.join().expect("publish second module");
    first.join().expect("publish first module");

    assert!(store.is_complete());
    assert_eq!(store.get(first_id).expect("first module").name, "first");
    let first_ptr = store.get(first_id).expect("first module") as *const BackendModule;
    let program = BackendProgram::from_module_store(std::sync::Arc::clone(&store));
    assert_eq!(&program.modules[0] as *const BackendModule, first_ptr);
    assert_eq!(
        program
            .modules
            .iter()
            .map(|module| module.id)
            .collect::<Vec<_>>(),
        vec![first_id, second_id]
    );
}

#[test]
#[should_panic(expected = "duplicate module owner")]
fn backend_module_store_rejects_duplicate_registered_owners() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();

    let _ = BackendModuleStore::new([module_id, module_id]);
}

#[test]
#[should_panic(expected = "was published twice")]
fn backend_module_store_rejects_duplicate_publication() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let ty = type_store
        .append_for_module(module_id)
        .primitive(PrimitiveTy::I32);
    let store = BackendModuleStore::new([module_id]);
    store.publish(module_with_global(module_id, ty, "first", false));

    store.publish(module_with_global(module_id, ty, "second", false));
}

#[test]
fn backend_module_readiness_delivers_publish_order_and_terminal_state() {
    let mut module_ids = ModuleIdAllocator::new();
    let first_id = module_ids.allocate();
    let second_id = module_ids.allocate();
    let type_store = nia_ty::TypeStore::new();
    let first_ty = type_store
        .append_for_module(first_id)
        .primitive(PrimitiveTy::I32);
    let second_ty = type_store
        .append_for_module(second_id)
        .primitive(PrimitiveTy::I32);
    let store = std::sync::Arc::new(BackendModuleStore::new([first_id, second_id]));
    let mut readiness = store.take_readiness();
    let publisher = std::sync::Arc::clone(&store);

    let publish = std::thread::spawn(move || {
        publisher.publish(module_with_global(second_id, second_ty, "second", false));
        publisher.publish(module_with_global(first_id, first_ty, "first", false));
    });

    assert_eq!(
        readiness.wait_next(),
        Some(BackendModuleReady {
            position: 1,
            module_id: second_id,
        })
    );
    assert_eq!(
        readiness.wait_next(),
        Some(BackendModuleReady {
            position: 0,
            module_id: first_id,
        })
    );
    assert_eq!(readiness.wait_next(), None);
    publish.join().expect("publish backend modules");
}

#[test]
#[should_panic(expected = "readiness already has a consumer")]
fn backend_module_readiness_rejects_second_consumer() {
    let store = std::sync::Arc::new(BackendModuleStore::new([]));
    let _readiness = store.take_readiness();

    let _second = store.take_readiness();
}

fn incremental_link_input(path: &str, key: CodegenUnitKey) -> IncrementalLinkInput<String> {
    IncrementalLinkInput {
        key,
        fingerprint: CodegenUnitFingerprint::from_parts([1, 2]),
        object: path.to_string(),
    }
}

#[test]
fn incremental_link_inputs_accept_strict_stable_key_order() {
    let inputs = IncrementalLinkInputs::new(vec![
        incremental_link_input(
            "main.o",
            CodegenUnitKey::SourceModule {
                source_identity: SourceIdentity::new("main.nia"),
                ordinal: 0,
            },
        ),
        incremental_link_input("builtins.o", CodegenUnitKey::CompilerBuiltins),
    ]);

    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs.as_slice()[0].object, "main.o");
    assert_eq!(inputs.into_vec()[1].object, "builtins.o");
}

#[test]
fn empty_incremental_link_inputs_are_valid() {
    let inputs = IncrementalLinkInputs::<String>::default();

    assert!(inputs.is_empty());
    assert!(inputs.as_slice().is_empty());
}

#[test]
#[should_panic(expected = "unique stable keys in ascending order")]
fn incremental_link_inputs_reject_duplicate_keys() {
    let _ = IncrementalLinkInputs::new(vec![
        incremental_link_input("first.o", CodegenUnitKey::CompilerBuiltins),
        incremental_link_input("second.o", CodegenUnitKey::CompilerBuiltins),
    ]);
}

#[test]
#[should_panic(expected = "unique stable keys in ascending order")]
fn incremental_link_inputs_reject_descending_keys() {
    let _ = IncrementalLinkInputs::new(vec![
        incremental_link_input("builtins.o", CodegenUnitKey::CompilerBuiltins),
        incremental_link_input(
            "main.o",
            CodegenUnitKey::SourceModule {
                source_identity: SourceIdentity::new("main.nia"),
                ordinal: 0,
            },
        ),
    ]);
}
