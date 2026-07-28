use super::*;

#[test]
fn extension_trait_facts_roundtrip_rehydrates_stable_owners() {
    let root = temp_dir("extension_trait_facts_rehydrate");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
    let source = crate::source_content_fingerprint("extension trait facts fixture");
    let dependency_source = crate::source_content_fingerprint("dependency fixture");
    let program_sources = crate::frontend_program_source_fingerprint([
        (&module, source, 512),
        (&dependency, dependency_source, 128),
    ]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let key =
        crate::FrontendExtensionTraitSolvingFactsCacheKey::new(namespace, &module, program_sources);

    let mut old_ids = ModuleIdAllocator::new();
    let old_module = old_ids.allocate();
    let old_dependency = old_ids.allocate();
    let old_store = TypeStore::new();
    let append = old_store.append_for_module(old_module);
    let old_symbols = SymbolTable::new();
    let generic_name = old_symbols.intern("Item").expect("intern generic");
    let associated_name = old_symbols.intern("Output").expect("intern associated");
    let value_name = old_symbols.intern("VALUE").expect("intern value");
    let primitive = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
    let generic = append.intern(TyKind::GenericParam(generic_name));
    let target_ty = append.intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(31),
        },
        args: vec![generic],
        const_args: Vec::new(),
    });
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: old_dependency,
        def_id: DefId(37),
    });
    let facts = CachedExtensionTraitSolvingFacts {
        trait_impls: vec![item_signatures::ProgramTraitImplSignature {
            module_id: old_module,
            impl_id: TraitImplId(41),
            builtin: Some("iterator".to_string()),
            generics: vec![generic_name],
            target_ty,
            trait_id,
            trait_args: vec![primitive],
            trait_const_args: vec![ConstGenericArg {
                ty: primitive,
                value: ConstGenericValue::Int(IntConst::unsigned(8)),
            }],
            where_predicates: vec![item_signatures::WherePredicateSignature {
                ty: generic,
                bounds: vec![item_signatures::WhereBoundSignature {
                    trait_ty: primitive,
                    associated_type_bindings: vec![
                        item_signatures::AssociatedTypeBindingSignature {
                            name: associated_name,
                            ty: target_ty,
                            span: nia_span::Span::new(20, 30),
                        },
                    ],
                    span: nia_span::Span::new(15, 31),
                }],
                span: nia_span::Span::new(10, 32),
            }],
            associated_types: vec![item_signatures::TraitImplAssociatedTypeSignature {
                name: associated_name,
                ty: target_ty,
                span: nia_span::Span::new(40, 50),
            }],
            associated_values: vec![item_signatures::TraitImplAssociatedValueSignature {
                def_id: DefId(43),
                name: value_name,
                visibility: Visibility::PublicPkg,
                span: nia_span::Span::new(60, 70),
            }],
        }],
        invalid_trait_impl_method_ids: HashSet::from([GlobalDefId {
            module_id: old_module,
            def_id: DefId(47),
        }]),
    };
    let old_paths = HashMap::from([
        (old_module, "src/main.nia".to_string()),
        (old_dependency, "src/dep.nia".to_string()),
    ]);
    let old_payload =
        encode_extension_trait_solving_facts(&facts, &module, &old_paths, &old_symbols, &old_store)
            .expect("encode old extension facts");
    cache
        .publish_extension_trait_solving_facts(
            ExtensionTraitSolvingFactsIdentity {
                key,
                namespace,
                module: &module,
                program_sources,
                source_len: 512,
            },
            &facts,
            &old_paths,
            &old_symbols,
            &old_store,
            false,
        )
        .expect("publish extension facts");

    let mut new_ids = ModuleIdAllocator::new();
    let new_dependency = new_ids.allocate();
    let new_module = new_ids.allocate();
    let new_store = TypeStore::new();
    let new_symbols = SymbolTable::new();
    let modules = HashMap::from([
        ("src/main.nia".to_string(), new_module),
        ("src/dep.nia".to_string(), new_dependency),
    ]);
    let loaded = cache
        .load_extension_trait_solving_facts(
            ExtensionTraitSolvingFactsIdentity {
                key,
                namespace,
                module: &module,
                program_sources,
                source_len: 512,
            },
            &modules,
            &new_symbols,
            &new_store,
        )
        .expect("load extension facts");
    let ExtensionTraitSolvingFactsLookup::Hit(loaded) = loaded else {
        panic!("expected extension facts cache hit");
    };
    assert_eq!(loaded.trait_impls[0].module_id, new_module);
    assert!(loaded.invalid_trait_impl_method_ids.contains(&GlobalDefId {
        module_id: new_module,
        def_id: DefId(47),
    }));
    assert!(matches!(
        loaded.trait_impls[0].trait_id,
        TraitId::Source(def_id) if def_id.module_id == new_dependency
    ));
    assert!(loaded.trait_impls.iter().all(|signature| {
        signature.target_ty.store_id == new_store.id()
            && signature
                .trait_args
                .iter()
                .all(|ty| ty.store_id == new_store.id())
    }));
    assert_eq!(new_symbols.resolve(generic_name).as_deref(), Some("Item"));
    assert_eq!(
        new_symbols.resolve(associated_name).as_deref(),
        Some("Output")
    );
    assert_eq!(new_symbols.resolve(value_name).as_deref(), Some("VALUE"));
    let new_paths = HashMap::from([
        (new_module, "src/main.nia".to_string()),
        (new_dependency, "src/dep.nia".to_string()),
    ]);
    let new_payload = encode_extension_trait_solving_facts(
        &loaded,
        &module,
        &new_paths,
        &new_symbols,
        &new_store,
    )
    .expect("encode rehydrated extension facts");
    assert_eq!(old_payload, new_payload);

    let path = cache.extension_trait_solving_facts_path(key);
    let mut corrupt = fs::read(&path).expect("read extension facts entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt extension facts entry");
    assert_eq!(
        cache
            .load_extension_trait_solving_facts(
                ExtensionTraitSolvingFactsIdentity {
                    key,
                    namespace,
                    module: &module,
                    program_sources,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load corrupt extension facts"),
        ExtensionTraitSolvingFactsLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extension_validation_diagnostics_roundtrip_and_retire_corruption() {
    let root = temp_dir("extension_validation_diagnostics");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let source = crate::source_content_fingerprint("extend ! { fn invalid(self) void {} }");
    let program_sources = crate::frontend_program_source_fingerprint([(&module, source, 128)]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let key = crate::FrontendExtensionValidationDiagnosticsCacheKey::new(
        namespace,
        &module,
        program_sources,
    );
    let identity = ExtensionValidationDiagnosticsIdentity {
        key,
        namespace,
        module: &module,
        program_sources,
        source_len: 128,
    };
    let diagnostics = vec![Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        nia_span::Span::new(7, 18),
        "extend target must be an extendable value type",
    )];

    cache
        .publish_extension_validation_diagnostics(identity, &diagnostics, false)
        .expect("publish validation diagnostics");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load validation diagnostics"),
        ExtensionValidationDiagnosticsLookup::Hit(diagnostics.clone())
    );

    let complete = Diagnostic::user_error(codes::NAME_RESOLUTION, "complete diagnostic shape")
        .primary(nia_span::Span::new(1, 2), "primary")
        .secondary(nia_span::Span::new(3, 4), "secondary")
        .note("note")
        .help("help")
        .related(nia_span::Span::new(5, 6), "related")
        .debug("owner", 7)
        .finish();
    cache
        .publish_extension_validation_diagnostics(identity, std::slice::from_ref(&complete), true)
        .expect("publish complete validation diagnostic");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load complete validation diagnostic"),
        ExtensionValidationDiagnosticsLookup::Hit(vec![complete])
    );

    let invalid_span = Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        nia_span::Span::new(127, 129),
        "invalid span",
    );
    assert!(
        cache
            .publish_extension_validation_diagnostics(identity, &[invalid_span], true)
            .is_err()
    );

    let path = cache.extension_validation_diagnostics_path(key);
    let mut corrupt = fs::read(&path).expect("read validation diagnostics entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt validation diagnostics entry");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load corrupt validation diagnostics"),
        ExtensionValidationDiagnosticsLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
