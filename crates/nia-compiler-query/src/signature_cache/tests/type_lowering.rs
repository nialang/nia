use super::*;

#[test]
fn type_lowering_roundtrip_rehydrates_canonical_type_graph() {
    let root = temp_dir("type_lowering_rehydrate");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
    let source = crate::source_content_fingerprint("signature lowering fixture");
    let dependency_source = crate::source_content_fingerprint("dependency fixture");
    let program_sources = crate::frontend_program_source_fingerprint([
        (&module, source, 512),
        (&dependency, dependency_source, 128),
    ]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeSpec::Bare,
    );
    let key = crate::FrontendSignatureTypeLoweringCacheKey::new(
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
    let old_store = TypeStore::new().expect("create type store");
    let append = old_store.append_for_module(old_module);
    let old_symbols = SymbolTable::new();
    let length = old_symbols.intern("Length").expect("intern length");
    let item = old_symbols.intern("Item").expect("intern item");
    let primitive = append.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let generic = append.test_intern(TyKind::GenericParam(length));
    let pointer = append.test_intern(TyKind::Pointer {
        is_readonly: true,
        elem: primitive,
    });
    let array = append.test_intern(TyKind::Array {
        len: ArrayLenTy::Builtin {
            builtin: LayoutBuiltin::Align,
            ty: pointer,
        },
        elem: generic,
    });
    let nominal = append.test_intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(31),
        },
        args: vec![array],
        const_args: vec![
            ConstGenericArg {
                ty: primitive,
                value: ConstGenericValue::GenericParam(length),
            },
            ConstGenericArg {
                ty: primitive,
                value: ConstGenericValue::Int(IntConst::unsigned(129)),
            },
            ConstGenericArg {
                ty: primitive,
                value: ConstGenericValue::Bool(true),
            },
            ConstGenericArg {
                ty: primitive,
                value: ConstGenericValue::Char('λ'),
            },
        ],
    });
    let trait_id = TraitId::Source(GlobalDefId {
        module_id: old_dependency,
        def_id: DefId(37),
    });
    let object = append.test_intern(TyKind::TraitObject {
        is_readonly: false,
        trait_id,
        trait_args: vec![nominal],
        trait_const_args: Vec::new(),
        associated_type_bindings: vec![AssociatedTypeBindingTy {
            trait_id: Some(TraitId::Builtin(BuiltinTrait::Iterator)),
            trait_args: vec![primitive],
            trait_const_args: Vec::new(),
            name: item,
            ty: array,
        }],
    });
    let projection = append.test_intern(TyKind::Projection {
        self_ty: object,
        trait_id,
        trait_args: vec![nominal],
        trait_const_args: Vec::new(),
        name: item,
    });
    let function = append.test_intern(TyKind::FunctionPointer {
        params: vec![projection, append.test_intern(TyKind::SelfParam)],
        return_type: append.test_intern(TyKind::Optional { elem: nominal }),
        is_variadic: true,
    });
    let opaque = append.test_intern(TyKind::Opaque);
    let tuple = append.test_intern(TyKind::Tuple(vec![primitive, opaque, array]));
    let roots = [
        function,
        tuple,
        append.test_intern(TyKind::Tuple(Vec::new())),
        append.test_intern(TyKind::ConstOnly),
        append.test_intern(TyKind::VolatilePointer {
            is_readonly: false,
            elem: primitive,
        }),
        append.test_intern(TyKind::Slice {
            is_readonly: true,
            elem: nominal,
        }),
        append.test_intern(TyKind::SlicePointee { elem: primitive }),
        append.test_intern(TyKind::Vector {
            elem: PrimitiveTy::I32,
            lanes: 8,
        }),
        append.test_intern(TyKind::Range {
            kind: RangeTyKind::ToInclusive,
            bound: Some(primitive),
        }),
        append.test_intern(TyKind::ErrorUnion {
            error: append.test_intern(TyKind::Error),
            value: nominal,
        }),
        append.test_intern(TyKind::BuiltinType(BuiltinType::AsmConfig)),
        append.test_intern(TyKind::BuiltinType(BuiltinType::AsmInputs)),
        append.test_intern(TyKind::BuiltinType(BuiltinType::AsmOutputs)),
        append.test_intern(TyKind::BuiltinTrait {
            trait_id: BuiltinTrait::Sized,
            args: vec![nominal],
        }),
        append.test_intern(TyKind::TraitObjectPointee {
            trait_id,
            trait_args: vec![primitive],
            trait_const_args: Vec::new(),
            associated_type_bindings: Vec::new(),
        }),
        append.test_intern(TyKind::Callable {
            is_readonly: false,
            params: vec![primitive, nominal],
            return_type: array,
        }),
        append.test_intern(TyKind::CallablePointee {
            params: vec![nominal, primitive],
            return_type: tuple,
        }),
    ];
    let type_uses = roots
        .into_iter()
        .enumerate()
        .map(|(index, ty)| {
            (
                NodeSite {
                    source_id: old_version.id,
                    kind: SyntaxKind::Type,
                    position: NodePosition::Span(nia_span::Span::new(index * 4, index * 4 + 3)),
                },
                ty,
            )
        })
        .collect();
    let lowering = TypeLowering {
        type_uses,
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let old_paths = HashMap::from([
        (old_module, "src/main.nia".to_string()),
        (old_dependency, "src/dep.nia".to_string()),
    ]);
    let old_payload =
        encode_type_lowering(&lowering, old_version, &old_paths, &old_symbols, &old_store)
            .expect("encode old lowering");
    cache
        .publish_type_lowering(
            SignatureTypeLoweringIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_version: old_version,
                source_len: 512,
            },
            &lowering,
            &old_paths,
            &old_symbols,
            &old_store,
            false,
        )
        .expect("publish lowering");

    let new_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let new_dependency = new_ids.allocate().expect("allocate module ID");
    let new_module = new_ids.allocate().expect("allocate module ID");
    let new_version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision(2),
    };
    let new_store = TypeStore::new().expect("create type store");
    let new_symbols = SymbolTable::new();
    let modules = HashMap::from([
        ("src/main.nia".to_string(), new_module),
        ("src/dep.nia".to_string(), new_dependency),
    ]);
    let cache = PersistentSignatureCache::new(root.clone());
    let loaded = cache
        .load_type_lowering(
            SignatureTypeLoweringIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_version: new_version,
                source_len: 512,
            },
            &modules,
            &new_symbols,
            &new_store,
        )
        .expect("load lowering");
    let SignatureTypeLoweringLookup::Hit(loaded) = loaded else {
        panic!("expected lowering cache hit");
    };
    assert!(
        loaded
            .type_uses
            .keys()
            .all(|site| site.source_id == new_version.id)
    );
    assert!(
        loaded
            .type_uses
            .values()
            .all(|ty| ty.store_id == new_store.id())
    );
    assert_eq!(new_symbols.resolve(length).as_deref(), Some("Length"));
    assert_eq!(new_symbols.resolve(item).as_deref(), Some("Item"));
    let new_paths = HashMap::from([
        (new_module, "src/main.nia".to_string()),
        (new_dependency, "src/dep.nia".to_string()),
    ]);
    let new_payload =
        encode_type_lowering(&loaded, new_version, &new_paths, &new_symbols, &new_store)
            .expect("encode rehydrated lowering");
    assert_eq!(old_payload, new_payload);

    let path = cache.type_lowering_path(key);
    let mut corrupt = fs::read(&path).expect("read lowering entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt lowering entry");
    assert_eq!(
        cache
            .load_type_lowering(
                SignatureTypeLoweringIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: new_version,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load corrupt lowering"),
        SignatureTypeLoweringLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn type_lowering_rejects_const_expression_handles() {
    let ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = ids.allocate().expect("allocate module ID");
    let store = TypeStore::new().expect("create type store");
    let append = store.append_for_module(module_id);
    let primitive = append.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
    let array = append.test_intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(GlobalConstExprId {
            module_id,
            const_expr_id: ConstExprId(3),
        }),
        elem: primitive,
    });
    let version = SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision::INITIAL,
    };
    let lowering = TypeLowering {
        type_uses: HashMap::from([(
            NodeSite {
                source_id: version.id,
                kind: SyntaxKind::Type,
                position: NodePosition::Span(nia_span::Span::new(0, 2)),
            },
            array,
        )]),
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    };
    assert!(
        encode_type_lowering(
            &lowering,
            version,
            &HashMap::from([(module_id, "src/main.nia".to_string())]),
            &SymbolTable::new(),
            &store,
        )
        .is_err()
    );
}
