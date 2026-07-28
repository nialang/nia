use super::*;

#[test]
fn executable_value_ref_edges_rehydrate_current_modules_and_retire_corruption() {
    let root = temp_dir("executable_value_ref_edges");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
    let source = crate::source_content_fingerprint("fn main() i32 { dep() }");
    let dependency_source = crate::source_content_fingerprint("fn dep() i32 { 1 }");
    let program_sources = crate::frontend_program_source_fingerprint([
        (&module, source, 24),
        (&dependency, dependency_source, 18),
    ]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let owner = DefId(11);
    let key = crate::FrontendExecutableValueRefEdgesCacheKey::new(
        namespace,
        &module,
        owner,
        program_sources,
    );
    let identity = ExecutableValueRefEdgesIdentity {
        key,
        namespace,
        module: &module,
        owner,
        program_sources,
    };

    let mut old_ids = ModuleIdAllocator::new();
    let old_module = old_ids.allocate();
    let old_dependency = old_ids.allocate();
    let old_paths = HashMap::from([
        (old_module, "src/main.nia".to_string()),
        (old_dependency, "src/dep.nia".to_string()),
    ]);
    let edges = CachedExecutableValueRefEdges {
        functions: HashSet::from([
            GlobalDefId {
                module_id: old_module,
                def_id: DefId(3),
            },
            GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(5),
            },
        ]),
        globals: HashSet::from([GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(7),
        }]),
    };
    cache
        .publish_executable_value_ref_edges(identity, &edges, &old_paths, false)
        .expect("publish executable value-ref edges");

    let mut new_ids = ModuleIdAllocator::new();
    let new_dependency = new_ids.allocate();
    let new_module = new_ids.allocate();
    let new_modules = HashMap::from([
        ("src/main.nia".to_string(), new_module),
        ("src/dep.nia".to_string(), new_dependency),
    ]);
    let loaded = cache
        .load_executable_value_ref_edges(identity, &new_modules)
        .expect("load executable value-ref edges");
    assert_eq!(
        loaded,
        ExecutableValueRefEdgesLookup::Hit(CachedExecutableValueRefEdges {
            functions: HashSet::from([
                GlobalDefId {
                    module_id: new_module,
                    def_id: DefId(3),
                },
                GlobalDefId {
                    module_id: new_dependency,
                    def_id: DefId(5),
                },
            ]),
            globals: HashSet::from([GlobalDefId {
                module_id: new_dependency,
                def_id: DefId(7),
            }]),
        })
    );

    let mut malformed_payload = Vec::new();
    write_u64(&mut malformed_payload, 2);
    for _ in 0..2 {
        write_string(&mut malformed_payload, "src/main.nia");
        write_u64(&mut malformed_payload, 3);
    }
    write_u64(&mut malformed_payload, 0);
    let path = cache.executable_value_ref_edges_path(key);
    fs::write(
        &path,
        encode_executable_value_ref_edges_entry(identity, &malformed_payload),
    )
    .expect("write malformed executable value-ref edges");
    assert_eq!(
        cache
            .load_executable_value_ref_edges(identity, &new_modules)
            .expect("reject malformed executable value-ref edges"),
        ExecutableValueRefEdgesLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
