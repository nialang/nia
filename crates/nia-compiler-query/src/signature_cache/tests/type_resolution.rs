use super::*;

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
        crate::RuntimeModel::Bare,
    );
    let key = crate::FrontendSignatureTypeResolutionCacheKey::new(
        namespace,
        &module,
        SignatureItemSet::Types,
        program_sources,
    );

    let mut old_ids = ModuleIdAllocator::new();
    let old_module = old_ids.allocate();
    let old_dependency = old_ids.allocate();
    let old_version = SourceVersion {
        id: SourceId(3),
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
    let mut const_names = NodeMap::builder(&old_store);
    const_names.insert(
        VersionedNodeKey {
            site: const_site.clone(),
            revision: old_version.revision,
        },
        generic,
    );
    let resolution = TypeResolution {
        node_type_names: HashMap::from([
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
        node_qualified_type_names: HashMap::from([(
            qualified_site,
            GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(41),
            },
        )]),
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

    let mut new_ids = ModuleIdAllocator::new();
    let new_dependency = new_ids.allocate();
    let new_module = new_ids.allocate();
    let new_version = SourceVersion {
        id: SourceId(90),
        revision: SourceRevision(2),
    };
    let new_store = nia_node_id::NodeStore::new();
    let new_symbols = SymbolTable::new();
    let modules = HashMap::from([
        ("src/main.nia".to_string(), new_module),
        ("src/dep.nia".to_string(), new_dependency),
    ]);
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
