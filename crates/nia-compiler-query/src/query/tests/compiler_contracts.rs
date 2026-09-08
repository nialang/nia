// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn compiler_query_registry_covers_all_declared_query_contracts() {
    let descriptors = compiler_query_registry().descriptors();

    assert_eq!(descriptors.len(), 138);
    assert!(
        !descriptors
            .iter()
            .any(|descriptor| descriptor.name == "module_graph_node")
    );
    for name in [
        "body_activation_worklist",
        "executable_fact_epoch",
        "module_graph_entry",
        "module_graph_path",
        "module_graph_parent",
        "module_graph_child",
        "module_package_root",
        "provider_fact_revision",
        "provider_fact_worklist",
    ] {
        assert!(
            descriptors.iter().any(|descriptor| descriptor.name == name),
            "missing precise graph fact query `{name}`"
        );
    }
    assert!(
        descriptors
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(descriptors.iter().all(|descriptor| {
        let expected_storage = if matches!(
            descriptor.name,
            "backend_item_plan"
                | "backend_module_item_plan"
                | "backend_module_finalization"
                | "compiled_package_type_roots"
                | "compiled_package_declarations"
                | "compiled_package_module_interface"
                | "compiled_package_templates"
                | "compiled_package_native"
        ) {
            nia_query::QueryStoragePolicy::SingleConsumerOwned
        } else {
            nia_query::QueryStoragePolicy::CacheOwnedArc
        };
        let expected_provider = if matches!(
            descriptor.name,
            "backend_module_item_plan"
                | "compiled_package_type_roots"
                | "compiled_package_declarations"
                | "compiled_package_module_interface"
                | "compiled_package_templates"
                | "compiled_package_native"
        ) {
            nia_query::QueryProviderPolicy::ExternallyPublished
        } else {
            nia_query::QueryProviderPolicy::KeyExecute
        };
        descriptor.context_type == std::any::type_name::<CompilerContext>()
            && descriptor.provider == expected_provider
            && descriptor.storage == expected_storage
    }));
    for descriptor in descriptors {
        let expected = match descriptor.name {
            "extension_provider_module_ids"
            | "extension_provider_module_eligibility"
            | "extension_provider_summary"
            | "compiled_package_interface_index"
            | "loaded_modules"
            | "module_graph_child"
            | "module_graph_entry"
            | "module_graph_parent"
            | "module_graph_path"
            | "module_package_root"
            | "module_path"
            | "module_source_version"
            | "parse_ok_module_ids"
            | "program_signature_module_eligibility"
            | "program_signature_module_ids"
            | "provider_fact_revision"
            | "provider_fact_worklist"
            | "public_surface_module"
            | "semantic_module_ids"
            | "using_scope_module" => nia_query::QueryFingerprintPolicy::StableValue,
            "active_module_item_tree_input"
            | "backend_module_function_instance_plan"
            | "backend_module_source_item_plan"
            | "body_activation_worklist"
            | "declaration_active_module_item_tree_input"
            | "declaration_module_item_tree_input"
            | "executable_function_body"
            | "executable_static_init"
            | "full_active_module_item_tree_input"
            | "executable_fact_epoch"
            | "full_module_item_tree_input"
            | "lowered_function_body"
            | "module_public_surface"
            | "module_item_tree_input"
            | "module_origins"
            | "module_parse_errors"
            | "module_using_scope"
            | "public_surface_module_facts"
            | "public_surface_type"
            | "public_surface_value"
            | "public_surfaces"
            | "public_using_scopes"
            | "signature_const_item_tree"
            | "signature_item_tree"
            | "using_scope_type"
            | "using_scope_unresolved"
            | "using_scope_value" => nia_query::QueryFingerprintPolicy::SemanticValue,
            _ => nia_query::QueryFingerprintPolicy::None,
        };
        assert_eq!(descriptor.fingerprint, expected, "{}", descriptor.name);
    }
}

#[test]
fn package_interface_publication_is_canonical_and_stable() {
    let fixture = LoadedProgramFixture::new(
        "src/main.nia",
        "pub fn greet() () {}\nfn private() () {}\npub struct User {}",
    );
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let first = database.package_interface_section(package.clone()).unwrap();
    let second = database.package_interface_section(package).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.records.len(), 2);
    assert!(
        first
            .records
            .windows(2)
            .all(|pair| { pair[0].definition < pair[1].definition })
    );
    assert!(
        first.records.iter().all(|record| {
            record.definition.name == "greet" || record.definition.name == "User"
        })
    );
    assert!(
        first
            .records
            .iter()
            .all(|record| nia_package_metadata::decode_declaration(&record.declaration).is_ok())
    );
}

#[test]
fn package_artifact_publication_round_trips_manifest_and_interface() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub fn greet() () {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let publication = database.publish_package_artifact(package.clone()).unwrap();
    assert_eq!(publication.manifest.package, package);
    let artifact = nia_package_metadata::PackageArtifact::open(publication.bytes).unwrap();
    assert_eq!(artifact.manifest(), &publication.manifest);
    assert!(artifact.interface().unwrap().is_some());
}

#[test]
fn package_publication_excludes_definitions_owned_by_dependency_packages() {
    let mut fixture = LoadedProgramFixture::new("src/main.nia", "pub fn root() () {}");
    let dependency_module = fixture.add_child(
        fixture.entry_id(),
        "dependency",
        "deps/lib.nia",
        "pub fn foreign() () {}",
    );
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "root".into(),
        version: "1.0.0".into(),
    };
    let dependency = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "dependency".into(),
        version: "1.0.0".into(),
    };
    let interface = database
        .package_interface_section_with_resolver(package.clone(), &|global: GlobalDefId| {
            Ok(if global.module_id == dependency_module {
                dependency.clone()
            } else {
                package.clone()
            })
        })
        .unwrap();
    assert_eq!(interface.records.len(), 1);
    assert_eq!(interface.records[0].definition.name, "root");
    assert_eq!(interface.records[0].definition.module.package, package);
}

#[test]
fn compiler_update_invalidates_replaced_compiled_interfaces_without_graph_change() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let loader = TestLoaderFacts::new(
        fixture.program(),
        crate::ProviderFactSnapshot::empty(crate::ProviderFactRevision::default()),
    );
    let database = super::super::CompilerDatabase::new(
        CompileRequest::new(fixture.program()).with_loader_facts(loader.clone()),
    );
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "dep".into(),
        version: "1.0.0".into(),
    };
    let make_interface =
        |name: &str, template_body: &[u8], dependency_hash: [u8; 32], native_byte: u8| {
            let section = nia_package_metadata::InterfaceSection {
                records: vec![nia_package_metadata::InterfaceRecord {
                    definition: nia_package_metadata::DefinitionId {
                        module: nia_package_metadata::ModuleId {
                            package: package.clone(),
                            path: "src/lib.nia".into(),
                        },
                        name: name.into(),
                        kind: 2,
                    },
                    declaration: b"NIADECL01".to_vec(),
                    type_roots: Vec::new(),
                }],
            };
            let bytes = nia_package_metadata::encode_interface(&section).unwrap();
            let templates = nia_package_metadata::TemplateSection {
                records: vec![nia_package_metadata::TemplateRecord {
                    definition: section.records[0].definition.clone(),
                    body: template_body.to_vec(),
                    summary: b"summary".to_vec(),
                }],
            };
            let template_bytes = nia_package_metadata::encode_templates(&templates).unwrap();
            let native_target = nia_target_config::TargetConfig::host();
            let native = nia_package_metadata::NativeSection {
                target: nia_package_metadata::NativeTarget {
                    arch: native_target.arch,
                    vendor: native_target.vendor,
                    os: native_target.os,
                    env: native_target.env,
                    abi: native_target.abi,
                    endian: native_target.endian,
                    pointer_width: native_target.pointer_width,
                },
                profile: 0,
                optimization: 0,
                objects: vec![nia_package_metadata::NativeObject {
                    key: "unit".into(),
                    bytes: vec![native_byte],
                }],
            };
            let native_bytes = nia_package_metadata::encode_native(&native).unwrap();
            let mut manifest = nia_package_metadata::PackageManifest::current(package.clone());
            manifest
                .dependencies
                .push(nia_package_metadata::PackageDependency {
                    package: nia_package_metadata::PackageId {
                        namespace: "example".into(),
                        name: "base".into(),
                        version: "1.0.0".into(),
                    },
                    interface_hash: dependency_hash,
                });
            manifest
                .modules
                .push(nia_package_metadata::ModuleInterface {
                    path: "src/lib.nia".into(),
                    interface_hash: nia_package_metadata::interface_module_hash(
                        &section,
                        "src/lib.nia",
                    )
                    .unwrap(),
                });
            let artifact = nia_package_metadata::PackageArtifact::open(
                nia_package_metadata::encode_artifact(
                    &manifest,
                    &[
                        (nia_package_metadata::SectionKind::Interface, &bytes),
                        (
                            nia_package_metadata::SectionKind::Templates,
                            &template_bytes,
                        ),
                        (nia_package_metadata::SectionKind::Native, &native_bytes),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
            nia_package_metadata::CompiledPackageInterface::from_artifact(&artifact).unwrap()
        };
    loader.replace_compiled_interfaces(vec![make_interface("first", b"a", [0; 32], 0)]);
    let _ = database.compiled_package_interface_index().unwrap();
    loader.replace_compiled_interfaces(vec![make_interface("first", b"a", [1; 32], 0)]);
    let dependency_invalidation = database
        .update(CompileRequest::new(fixture.program()).with_loader_facts(loader.clone()))
        .unwrap();
    assert!(
        dependency_invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "compiled_package_interface_index")
    );
    loader.replace_compiled_interfaces(vec![make_interface("first", b"b", [1; 32], 0)]);
    let template_invalidation = database
        .update(CompileRequest::new(fixture.program()).with_loader_facts(loader.clone()))
        .unwrap();
    assert!(
        template_invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "compiled_package_interface_index")
    );
    loader.replace_compiled_interfaces(vec![make_interface("first", b"b", [1; 32], 1)]);
    let native_invalidation = database
        .update(CompileRequest::new(fixture.program()).with_loader_facts(loader.clone()))
        .unwrap();
    assert!(
        native_invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "compiled_package_interface_index")
    );
}

#[test]
fn stable_type_graph_publication_remaps_session_handles() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub fn greet() Unit {}");
    let database = fixture.database();
    let append = database
        .db
        .context()
        .type_store
        .append_for_module(fixture.entry_id());
    let int_ty = append.primitive(nia_ty::PrimitiveTy::I32);
    let pointer_ty = append.intern(nia_ty::TyKind::Pointer {
        is_readonly: true,
        elem: int_ty,
    });
    let graph = database
        .stable_type_graph_for_roots(
            nia_package_metadata::PackageId {
                namespace: "example".into(),
                name: "demo".into(),
                version: "1.0.0".into(),
            },
            &[pointer_ty],
        )
        .unwrap();
    assert_eq!(graph.roots, vec![1]);
    assert_eq!(
        graph.nodes[0],
        nia_package_metadata::StableTypeNode::Primitive(3)
    );
    assert_eq!(
        graph.nodes[1],
        nia_package_metadata::StableTypeNode::Pointer {
            target: 0,
            readonly: true,
        }
    );
}

#[test]
fn stable_type_graph_order_is_independent_of_session_allocation_order() {
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let first = LoadedProgramFixture::new("src/main.nia", "pub fn greet() Unit {}");
    let first_db = first.database();
    let append = first_db
        .db
        .context()
        .type_store
        .append_for_module(first.entry_id());
    let i32_a = append.primitive(nia_ty::PrimitiveTy::I32);
    let ptr_a = append.intern(nia_ty::TyKind::Pointer {
        is_readonly: true,
        elem: i32_a,
    });
    let tuple_a = append.intern(nia_ty::TyKind::Tuple(vec![i32_a]));
    let graph_a = first_db
        .stable_type_graph_for_roots(package.clone(), &[ptr_a, tuple_a])
        .unwrap();

    let second = LoadedProgramFixture::new("src/main.nia", "pub fn greet() Unit {}");
    let second_db = second.database();
    let append = second_db
        .db
        .context()
        .type_store
        .append_for_module(second.entry_id());
    let i32_b = append.primitive(nia_ty::PrimitiveTy::I32);
    let tuple_b = append.intern(nia_ty::TyKind::Tuple(vec![i32_b]));
    let ptr_b = append.intern(nia_ty::TyKind::Pointer {
        is_readonly: true,
        elem: i32_b,
    });
    let graph_b = second_db
        .stable_type_graph_for_roots(package, &[ptr_b, tuple_b])
        .unwrap();

    assert_eq!(graph_a, graph_b);
    assert_eq!(
        nia_package_metadata::encode_type_graph(&graph_a).unwrap(),
        nia_package_metadata::encode_type_graph(&graph_b).unwrap()
    );
}

#[test]
fn stable_type_graph_publication_encodes_generic_parameter_identity() {
    let fixture =
        LoadedProgramFixture::new("src/main.nia", "pub fn identity[T](value: T) T { value }");
    let database = fixture.database();
    let append = database
        .db
        .context()
        .type_store
        .append_for_module(fixture.entry_id());
    let generic = append.intern(nia_ty::TyKind::GenericParam(sym("T")));
    let graph = database
        .stable_type_graph_for_roots(
            nia_package_metadata::PackageId {
                namespace: "example".into(),
                name: "demo".into(),
                version: "1.0.0".into(),
            },
            &[generic],
        )
        .unwrap();
    assert_eq!(
        graph.nodes,
        vec![nia_package_metadata::StableTypeNode::GenericParam(
            sym("T").raw()
        )]
    );
}

#[test]
fn stable_type_graph_rehydrates_into_current_type_store() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub struct User {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let graph = nia_package_metadata::StableTypeGraph {
        nodes: vec![
            nia_package_metadata::StableTypeNode::Primitive(3),
            nia_package_metadata::StableTypeNode::Pointer {
                target: 0,
                readonly: true,
            },
        ],
        roots: vec![1],
    };
    let roots = database
        .rehydrate_stable_type_graph(
            &graph,
            &|definition: &nia_package_metadata::DefinitionId| {
                assert_eq!(definition.module.package, package);
                Ok(nia_ids::GlobalDefId {
                    module_id: fixture.entry_id(),
                    def_id: nia_ids::DefId(0),
                })
            },
        )
        .unwrap();
    assert_eq!(roots.len(), 1);
    assert!(matches!(
        database.db.context().type_store.get(roots[0]),
        Some(nia_ty::TyKind::Pointer {
            is_readonly: true,
            ..
        })
    ));
}

#[test]
fn stable_type_graph_publication_remaps_nominal_definition_identity() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub struct User {}");
    let database = fixture.database();
    let module = fixture.entry_id();
    let defs = database.db.get(FullModuleDefsQuery(module)).unwrap();
    let (def_id, _) = defs.semantic.defs.iter().next().unwrap();
    let append = database.db.context().type_store.append_for_module(module);
    let nominal = append.intern(nia_ty::TyKind::Nominal {
        def_id: nia_ids::GlobalDefId {
            module_id: module,
            def_id,
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let graph = database
        .stable_type_graph_for_roots(
            nia_package_metadata::PackageId {
                namespace: "example".into(),
                name: "demo".into(),
                version: "1.0.0".into(),
            },
            &[nominal],
        )
        .unwrap();
    let nia_package_metadata::StableTypeNode::Named(definition) = &graph.nodes[0] else {
        panic!("nominal type must publish as a stable definition identity");
    };
    assert_eq!(definition.module.path, "src/main.nia");
    assert_eq!(definition.name, "User");
}

#[test]
fn stable_type_graph_publication_preserves_nominal_type_arguments() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub struct Box[T] {}");
    let database = fixture.database();
    let module = fixture.entry_id();
    let defs = database.db.get(FullModuleDefsQuery(module)).unwrap();
    let (def_id, _) = defs.semantic.defs.iter().next().unwrap();
    let append = database.db.context().type_store.append_for_module(module);
    let argument = append.primitive(nia_ty::PrimitiveTy::I32);
    let nominal = append.intern(nia_ty::TyKind::Nominal {
        def_id: nia_ids::GlobalDefId {
            module_id: module,
            def_id,
        },
        args: vec![argument],
        const_args: Vec::new(),
    });
    let graph = database
        .stable_type_graph_for_roots(
            nia_package_metadata::PackageId {
                namespace: "example".into(),
                name: "demo".into(),
                version: "1.0.0".into(),
            },
            &[nominal],
        )
        .unwrap();
    assert!(matches!(
        &graph.nodes[1],
        nia_package_metadata::StableTypeNode::NamedApplied { arguments, .. } if arguments == &vec![0]
    ));
}

#[test]
fn stable_type_graph_publication_uses_explicit_definition_package_resolver() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub struct User {}");
    let database = fixture.database();
    let module = fixture.entry_id();
    let dependency = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "dependency".into(),
        version: "2.0.0".into(),
    };
    let defs = database.db.get(FullModuleDefsQuery(module)).unwrap();
    let (def_id, _) = defs.semantic.defs.iter().next().unwrap();
    let append = database.db.context().type_store.append_for_module(module);
    let nominal = append.intern(nia_ty::TyKind::Nominal {
        def_id: nia_ids::GlobalDefId {
            module_id: module,
            def_id,
        },
        args: Vec::new(),
        const_args: Vec::new(),
    });
    let graph = database
        .stable_type_graph_for_roots_with_resolver(&[nominal], &|resolved: nia_ids::GlobalDefId| {
            assert_eq!(resolved.module_id, module);
            Ok(dependency.clone())
        })
        .unwrap();
    let nia_package_metadata::StableTypeNode::Named(definition) = &graph.nodes[0] else {
        panic!("nominal type must publish as a stable definition identity");
    };
    assert_eq!(definition.module.package, dependency);
}

#[test]
fn package_interface_publication_accepts_external_definition_resolver() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub fn greet() () {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "app".into(),
        version: "1.0.0".into(),
    };
    let interface = database
        .package_interface_section_with_resolver(package.clone(), &|_| Ok(package.clone()))
        .unwrap();
    assert!(
        interface
            .records
            .iter()
            .any(|record| record.definition.name == "greet")
    );
}

#[test]
fn stable_definition_index_remaps_current_session_identities() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub fn greet() () {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let index = database
        .stable_definition_index(&|_| Ok(package.clone()))
        .unwrap();
    let identity = nia_package_metadata::DefinitionId {
        module: nia_package_metadata::ModuleId {
            package,
            path: "src/main.nia".into(),
        },
        name: "greet".into(),
        kind: 2,
    };
    let resolved = index.definition_for_identity(&identity).unwrap();
    assert_eq!(resolved.module_id, fixture.entry_id());
    assert_eq!(index.len(), 1);
    let module = identity.module.clone();
    assert_eq!(module.package, identity.module.package);
    assert_eq!(module.path, "src/main.nia");
    assert_eq!(index.module(&module), Some(fixture.entry_id()));
    assert_eq!(index.module_len(), 1);
}

#[test]
fn stable_module_index_keeps_package_identity_in_the_lookup_key() {
    let first = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "first".into(),
        version: "1.0.0".into(),
    };
    let second = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "second".into(),
        version: "1.0.0".into(),
    };
    let mut index = StableModuleIndex::new();
    let mut allocator = nia_ids::ModuleIdAllocator::new();
    let first_module = allocator.allocate();
    let second_module = allocator.allocate();
    index.insert(
        nia_package_metadata::ModuleId {
            package: first.clone(),
            path: "src/lib.nia".into(),
        },
        first_module,
    );
    index.insert(
        nia_package_metadata::ModuleId {
            package: second.clone(),
            path: "src/lib.nia".into(),
        },
        second_module,
    );
    assert_eq!(index.len(), 2);
    assert_eq!(
        index.module(&nia_package_metadata::ModuleId {
            package: first,
            path: "src/lib.nia".into(),
        }),
        Some(first_module)
    );
    assert_eq!(
        index.module(&nia_package_metadata::ModuleId {
            package: second,
            path: "src/lib.nia".into(),
        }),
        Some(second_module)
    );
}

#[test]
fn stable_module_index_uses_explicit_module_package_resolution() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "fn main() () {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let entry = fixture.entry_id();
    let index = database
        .stable_module_index(&|module| {
            assert_eq!(module, entry);
            Ok(package.clone())
        })
        .unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(
        index.module(&nia_package_metadata::ModuleId {
            package,
            path: "src/main.nia".into(),
        }),
        Some(entry)
    );
}

#[test]
fn rehydrate_compiled_interface_type_roots_is_empty_without_selected_artifacts() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "fn main() () {}");
    let database = fixture.database();
    fn no_resolver(
        _: &nia_package_metadata::DefinitionId,
    ) -> nia_query::QueryResult<nia_ids::GlobalDefId> {
        panic!("resolver must not be called when no artifacts are selected")
    }
    let roots = database
        .rehydrate_compiled_interface_type_roots(&no_resolver)
        .unwrap();
    assert!(roots.is_empty());
}

#[test]
fn compiled_package_type_roots_reject_unselected_packages() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "fn main() () {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "dep".into(),
        version: "1.0.0".into(),
    };
    let identity = nia_package_metadata::DefinitionId {
        module: nia_package_metadata::ModuleId {
            package: package.clone(),
            path: "src/lib.nia".into(),
        },
        name: "answer".into(),
        kind: 2,
    };
    let mut roots = std::collections::BTreeMap::new();
    roots.insert(identity.clone(), Vec::new());
    let _ = database.compiled_package_interface_index().unwrap();
    assert!(
        database
            .publish_compiled_package_type_roots(package.clone(), roots)
            .is_err()
    );
    assert!(database.compiled_package_type_roots(&package).is_err());
}

#[test]
fn installing_compiled_interface_roots_is_a_noop_without_artifacts() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "fn main() () {}");
    let database = fixture.database();
    fn no_resolver(
        _: &nia_package_metadata::DefinitionId,
    ) -> nia_query::QueryResult<nia_ids::GlobalDefId> {
        panic!("resolver must not be called when no artifacts are selected")
    }
    assert!(
        database
            .install_compiled_interface_type_roots(&no_resolver)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn compiled_interface_index_resolves_stable_definitions_without_session_handles() {
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "dep".into(),
        version: "1.0.0".into(),
    };
    let interface = nia_package_metadata::InterfaceSection {
        records: vec![nia_package_metadata::InterfaceRecord {
            definition: nia_package_metadata::DefinitionId {
                module: nia_package_metadata::ModuleId {
                    package: package.clone(),
                    path: "src/lib.nia".into(),
                },
                name: "answer".into(),
                kind: 2,
            },
            declaration: b"NIADECL01".to_vec(),
            type_roots: Vec::new(),
        }],
    };
    let interface_bytes = nia_package_metadata::encode_interface(&interface).unwrap();
    let mut manifest = nia_package_metadata::PackageManifest::current(package.clone());
    manifest
        .modules
        .push(nia_package_metadata::ModuleInterface {
            path: "src/lib.nia".into(),
            interface_hash: nia_package_metadata::interface_module_hash(&interface, "src/lib.nia")
                .unwrap(),
        });
    let artifact = nia_package_metadata::PackageArtifact::open(
        nia_package_metadata::encode_artifact(
            &manifest,
            &[(
                nia_package_metadata::SectionKind::Interface,
                &interface_bytes,
            )],
        )
        .unwrap(),
    )
    .unwrap();
    let compiled =
        nia_package_metadata::CompiledPackageInterface::from_artifact(&artifact).unwrap();
    let index = CompiledPackageInterfaceIndex::from_interfaces(vec![compiled]).unwrap();
    assert_eq!(index.package(&package).unwrap().records().len(), 1);
    assert_eq!(
        index.module_records(&package, "src/lib.nia").unwrap().len(),
        1
    );
    assert!(
        index
            .module(&nia_package_metadata::ModuleId {
                package: package.clone(),
                path: "src/lib.nia".into(),
            })
            .is_some()
    );
    assert!(index.definition(&interface.records[0].definition).is_some());
}

#[test]
fn installing_compiled_templates_publishes_empty_package_slots() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "fn main() () {}");
    let database = fixture.database();
    assert!(
        database
            .install_compiled_package_templates()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn loaded_definition_resolver_validates_stable_identity() {
    let fixture = LoadedProgramFixture::new("src/main.nia", "pub struct User {}");
    let database = fixture.database();
    let package = nia_package_metadata::PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let interface = database.package_interface_section(package.clone()).unwrap();
    let definition = &interface.records[0].definition;
    let resolved = database
        .resolve_loaded_definition(definition, &package)
        .unwrap();
    assert_eq!(resolved.module_id, fixture.entry_id());
    assert_eq!(
        database
            .resolve_loaded_definition(
                definition,
                &nia_package_metadata::PackageId {
                    namespace: "other".into(),
                    name: "demo".into(),
                    version: "1.0.0".into(),
                }
            )
            .is_err(),
        true
    );
}

#[test]
fn public_options_flow_through_compiler_query_context() {
    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::O3,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
static zeroes: [i32; 4] = [0; 4];

fn main() i32 {
zeroes[0]
}
"#,
        );
        let checked =
            CompilerDatabase::new(CompileRequest::new(fixture.program()).with_optimization(level))
                .codegen_program();
        let policy = level.policy();

        assert!(
            checked.diagnostics.is_empty(),
            "{level:?}: {:?}",
            checked.diagnostics
        );
        assert_eq!(checked.optimization, policy, "{level:?}");
        assert_eq!(checked.backend_lowering.optimization, policy, "{level:?}");
        assert_eq!(
            checked
                .backend_lowering
                .optimization_report
                .enabled_global_passes,
            if policy.prefer_size || policy.const_fold.at_least(nia_opt::OptimizationDepth::Full) {
                vec!["simplify-static-init"]
            } else {
                Vec::new()
            },
            "{level:?}"
        );
    }
}

#[test]
fn compiler_database_exposes_query_trace() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let checked = database.check_program();
    let trace = database.query_trace();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "checked_program" && dependency.to.name == "checked_module_ids"
    }));
}

#[test]
fn compiler_update_rejects_untracked_snapshot_provider() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = crate::query::CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = database.update(CompileRequest::new(fixture.program()));
    }));

    assert!(result.is_err());
}

#[test]
fn compiler_query_providers_can_override_query_execution() {
    fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> QueryResult<StableModuleSequence> {
        Ok(StableModuleSequence::default())
    }

    let providers = CompilerQueryProviders {
        parse_ok_module_ids: no_parse_ok_modules,
        ..CompilerQueryProviders::default()
    };
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let checked =
        compiler_database_with_providers(CompileRequest::new(fixture.program()), providers)
            .codegen_program()
            .expect("overridden codegen program");

    assert!(checked.modules.is_empty());
}

#[test]
fn missing_loaded_module_id_propagates_query_failure() {
    fn unknown_module_id() -> ModuleId {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        module_ids.allocate();
        module_ids.allocate()
    }

    fn unknown_checked_module(_: &QueryDb<CompilerContext>) -> QueryResult<Vec<ModuleId>> {
        Ok(vec![unknown_module_id()])
    }

    let providers = CompilerQueryProviders {
        checked_module_ids: unknown_checked_module,
        ..CompilerQueryProviders::default()
    };
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = compiler_database_with_providers(
        CompileRequest::new(fixture.program()).with_optimization(NiaOptimizationLevel::Oz),
        providers,
    );
    let missing_module = unknown_module_id();
    for error in [
        database
            .db
            .get(ModulePathQuery(missing_module))
            .expect_err("missing module path should be a query error"),
        database
            .db
            .get(ModuleItemTreeQuery(missing_module))
            .expect_err("missing module item tree should propagate its input query error"),
        database
            .db
            .get(ModuleDefsQuery(missing_module))
            .expect_err("module definitions should propagate a missing item tree error"),
        database
            .db
            .get(TypeResolutionQuery(missing_module))
            .expect_err("type resolution should propagate a missing module input"),
        database
            .db
            .get(TypeNormalizationQuery(missing_module))
            .expect_err("type normalization should propagate a missing module input"),
        database
            .db
            .get(SemanticUseTableQuery(missing_module))
            .expect_err("semantic uses should propagate a missing module input"),
        database
            .db
            .get(ConstModuleQuery(missing_module))
            .expect_err("const lowering should propagate a missing module input"),
        database
            .db
            .get(SignatureConstModuleQuery(missing_module))
            .expect_err("signature const lowering should propagate a missing module input"),
        database
            .db
            .get(ConstArrayLengthsQuery(missing_module))
            .expect_err("const array lengths should propagate a missing module input"),
        database
            .db
            .get(ConstEnumValuesQuery(missing_module))
            .expect_err("const enum values should propagate a missing module input"),
        database
            .db
            .get(ConstValuesQuery(missing_module))
            .expect_err("const values should propagate a missing module input"),
        database
            .db
            .get(ConstTypedFactsQuery(missing_module))
            .expect_err("const typed facts should propagate a missing module input"),
        database
            .db
            .get(ConstQuery(missing_module))
            .expect_err("const checking should propagate a missing module input"),
        database
            .db
            .get(SignatureLayoutsQuery(missing_module))
            .expect_err("signature layouts should propagate a missing module input"),
        database
            .db
            .get(LayoutsQuery(missing_module))
            .expect_err("layouts should propagate a missing module input"),
        database
            .db
            .get(StaticCheckQuery(missing_module))
            .expect_err("static checking should propagate a missing module input"),
        database
            .db
            .get(BodyCheckQuery(missing_module))
            .expect_err("body checking should propagate a missing module input"),
        full_body_check_resolution_inputs(&database.db, missing_module)
            .err()
            .expect("body resolution inputs should propagate a missing module input"),
        database
            .db
            .get(CheckedModuleQuery(missing_module))
            .expect_err("checked modules should propagate a missing module input"),
        database
            .db
            .get(CheckedProgramQuery)
            .expect_err("checked program aggregation should propagate a missing module input"),
        database
            .db
            .get(FlowCheckQuery(missing_module))
            .expect_err("flow checking should propagate a missing module input"),
        database
            .db
            .get(ModuleAbiSignatureFactsQuery(missing_module))
            .expect_err("ABI signature facts should propagate a missing module input"),
        database
            .db
            .get(AbiCheckQuery(missing_module))
            .expect_err("ABI checking should propagate a missing module input"),
        database
            .db
            .get(ExtensionSignatureModuleInputQuery(missing_module))
            .expect_err("extension signature input should propagate a missing module input"),
        database
            .db
            .get(ExtensionTraitSolvingModuleFactsQuery(missing_module))
            .expect_err("extension trait facts should propagate a missing module input"),
        database
            .db
            .get(ExtensionProviderValidationFactsQuery(missing_module))
            .expect_err("extension validation should propagate a missing module input"),
        database
            .db
            .get(VisibleExtensionsQuery(missing_module))
            .expect_err("visible extensions should propagate a missing module input"),
        database
            .db
            .get(ExecutableValueRefEdgesQuery(GlobalDefId {
                module_id: missing_module,
                def_id: nia_ids::DefId(0),
            }))
            .expect_err("value-ref edges should propagate a missing module input"),
    ] {
        assert!(matches!(error, QueryError::InvalidInput { .. }));
        assert!(
            error
                .to_string()
                .contains(&format!("missing loaded module {missing_module:?}"))
        );
    }

    let error = database
        .analyze_program()
        .expect_err("public analysis must propagate a missing module query failure");
    assert!(matches!(error, QueryError::InvalidInput { .. }));
    assert!(
        error
            .to_string()
            .contains(&format!("missing loaded module {:?}", unknown_module_id()))
    );
}
