use super::*;

#[test]
fn type_resolution_encoding_is_independent_of_map_insertion_order() {
    let version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(4),
    };
    let sites = [
        NodeSite {
            source_id: version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::Span(nia_span::Span::new(256, 260)),
        },
        NodeSite {
            source_id: version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::Span(nia_span::Span::new(1, 5)),
        },
        NodeSite {
            source_id: version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::ChildPath(NodeChildPath::from_steps(vec![2, 1])),
        },
    ];
    let store = nia_node_id::NodeStore::new();
    let make_resolution = |indexes: &[usize]| {
        let mut node_type_names = nia_hash::FastHashMap::default();
        for index in indexes {
            node_type_names.insert(
                sites[*index].clone(),
                TypeNameResolution::Primitive(PrimitiveTypeSpelling::Scalar(PrimitiveTy::Usize)),
            );
        }
        TypeResolution {
            node_type_names,
            node_qualified_type_names: nia_hash::FastHashMap::default(),
            unresolved_type_candidates: nia_hash::FastHashSet::default(),
            node_const_generic_names: NodeMap::builder(&store).finish(),
            diagnostics: Vec::new(),
        }
    };

    let forward = encode_type_resolution(
        &make_resolution(&[0, 1, 2]),
        version,
        &HashMap::new(),
        &SymbolTable::new(),
    )
    .expect("encode forward insertion order");
    let reverse = encode_type_resolution(
        &make_resolution(&[2, 1, 0]),
        version,
        &HashMap::new(),
        &SymbolTable::new(),
    )
    .expect("encode reverse insertion order");
    assert_eq!(forward, reverse);
}

#[test]
fn type_resolution_encoding_rejects_foreign_source_and_revision() {
    let version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(4),
    };
    let store = nia_node_id::NodeStore::new();
    let foreign_site = NodeSite {
        source_id: SourceId::isolated(),
        kind: SyntaxKind::Type,
        position: NodePosition::Span(nia_span::Span::new(0, 1)),
    };
    let foreign_source = TypeResolution {
        node_type_names: nia_hash::FastHashMap::from_iter([(
            foreign_site,
            TypeNameResolution::Primitive(PrimitiveTypeSpelling::Scalar(PrimitiveTy::Usize)),
        )]),
        node_qualified_type_names: nia_hash::FastHashMap::default(),
        unresolved_type_candidates: nia_hash::FastHashSet::default(),
        node_const_generic_names: NodeMap::builder(&store).finish(),
        diagnostics: Vec::new(),
    };
    assert!(
        encode_type_resolution(
            &foreign_source,
            version,
            &HashMap::new(),
            &SymbolTable::new(),
        )
        .is_err()
    );

    let symbols = SymbolTable::new();
    let symbol = symbols.intern("Length").expect("intern symbol");
    let mut const_names = NodeMap::builder(&store);
    const_names.insert(
        VersionedNodeKey {
            site: NodeSite {
                source_id: version.id,
                kind: SyntaxKind::Expr,
                position: NodePosition::Span(nia_span::Span::new(2, 3)),
            },
            revision: SourceRevision(5),
        },
        symbol,
    );
    let foreign_revision = TypeResolution {
        node_type_names: nia_hash::FastHashMap::default(),
        node_qualified_type_names: nia_hash::FastHashMap::default(),
        unresolved_type_candidates: nia_hash::FastHashSet::default(),
        node_const_generic_names: const_names.finish(),
        diagnostics: Vec::new(),
    };
    assert!(encode_type_resolution(&foreign_revision, version, &HashMap::new(), &symbols).is_err());
}

#[test]
fn type_resolution_rehydrates_current_source_module_and_symbol_owners() {
    let root = temp_dir("type_resolution_rehydrate");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
    let source = crate::source_content_fingerprint("type Value = dep::Value");
    let dependency_source = crate::source_content_fingerprint("pub struct Value {}");
    let program_sources = crate::frontend_program_source_fingerprint([
        (&module, source, 23),
        (&dependency, dependency_source, 19),
    ]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeSpec::Bare,
    );
    let key = crate::FrontendSignatureTypeResolutionCacheKey::new(
        namespace,
        &module,
        SignatureItemSet::Types,
        program_sources,
    );

    let old_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let old_module = old_ids.allocate().expect("allocate module ID");
    let old_dependency = old_ids.allocate().expect("allocate module ID");
    let old_version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(7),
    };
    let old_store = nia_node_id::NodeStore::new();
    let old_symbols = SymbolTable::new();
    let generic = old_symbols.intern("Length").expect("intern symbol");
    let type_site = NodeSite {
        source_id: old_version.id,
        kind: SyntaxKind::Type,
        position: NodePosition::Span(nia_span::Span::new(5, 10)),
    };
    let qualified_site = NodeSite {
        source_id: old_version.id,
        kind: SyntaxKind::Type,
        position: NodePosition::ChildPath(NodeChildPath::from_steps(vec![1, 2, 3])),
    };
    let const_site = NodeSite {
        source_id: old_version.id,
        kind: SyntaxKind::Expr,
        position: NodePosition::ChildPathRange {
            start: NodeChildPath::from_steps(vec![4]),
            end: NodeChildPath::from_steps(vec![5]),
        },
    };
    let unresolved_candidate_site = NodeSite {
        source_id: old_version.id,
        kind: SyntaxKind::Type,
        position: NodePosition::Span(nia_span::Span::new(11, 16)),
    };
    let mut const_names = NodeMap::builder(&old_store);
    const_names.insert(
        VersionedNodeKey {
            site: const_site.clone(),
            revision: old_version.revision,
        },
        generic,
    );
    let resolution = TypeResolution {
        node_type_names: nia_hash::FastHashMap::from_iter([
            (
                type_site.clone(),
                TypeNameResolution::Primitive(PrimitiveTypeSpelling::Scalar(PrimitiveTy::Usize)),
            ),
            (
                qualified_site.clone(),
                TypeNameResolution::External(GlobalDefId {
                    module_id: old_dependency,
                    def_id: DefId(41),
                }),
            ),
        ]),
        node_qualified_type_names: nia_hash::FastHashMap::from_iter([(
            qualified_site,
            GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(41),
            },
        )]),
        unresolved_type_candidates: nia_hash::FastHashSet::from_iter([
            unresolved_candidate_site.clone()
        ]),
        node_const_generic_names: const_names.finish(),
        diagnostics: Vec::new(),
    };
    let old_paths = HashMap::from([
        (old_module, "src/main.nia".to_string()),
        (old_dependency, "src/dep.nia".to_string()),
    ]);
    cache
        .publish_type_resolution(
            SignatureTypeResolutionIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_version: old_version,
                source_len: 23,
            },
            &resolution,
            &old_paths,
            &old_symbols,
            false,
        )
        .expect("publish cache entry");

    let new_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let new_dependency = new_ids.allocate().expect("allocate module ID");
    let new_module = new_ids.allocate().expect("allocate module ID");
    let new_version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(2),
    };
    let new_store = nia_node_id::NodeStore::new();
    let new_symbols = SymbolTable::new();
    let modules = HashMap::from([
        ("src/main.nia".to_string(), new_module),
        ("src/dep.nia".to_string(), new_dependency),
    ]);
    let cache = PersistentSignatureCache::new(root.clone());
    let loaded = cache
        .load_type_resolution(
            SignatureTypeResolutionIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_version: new_version,
                source_len: 23,
            },
            &modules,
            &new_symbols,
            &new_store,
        )
        .expect("load cache entry");
    let SignatureTypeResolutionLookup::Hit(loaded) = loaded else {
        panic!("expected cache hit");
    };
    assert!(
        loaded
            .node_type_names
            .keys()
            .all(|site| site.source_id == new_version.id)
    );
    assert_eq!(
        loaded.node_type_names.get(&NodeSite {
            source_id: new_version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::ChildPath(NodeChildPath::from_steps(vec![1, 2, 3])),
        }),
        Some(&TypeNameResolution::External(GlobalDefId {
            module_id: new_dependency,
            def_id: DefId(41),
        }))
    );
    assert!(loaded.unresolved_type_candidates.contains(&NodeSite {
        source_id: new_version.id,
        ..unresolved_candidate_site
    }));
    let new_const_key = VersionedNodeKey {
        site: NodeSite {
            source_id: new_version.id,
            ..const_site
        },
        revision: new_version.revision,
    };
    let loaded_generic = loaded
        .node_const_generic_names
        .get(&new_const_key)
        .copied()
        .expect("rehydrated const generic");
    assert_eq!(
        new_symbols.resolve(loaded_generic).as_deref(),
        Some("Length")
    );
    assert_eq!(loaded.node_const_generic_names.store_id(), new_store.id());

    let path = cache.type_resolution_path(key);
    let mut corrupt = fs::read(&path).expect("read entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt entry");
    assert_eq!(
        cache
            .load_type_resolution(
                SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: new_version,
                    source_len: 23,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load corrupt entry"),
        SignatureTypeResolutionLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
