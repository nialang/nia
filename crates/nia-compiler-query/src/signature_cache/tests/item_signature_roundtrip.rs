use super::*;

#[test]
fn item_signatures_roundtrip_rehydrates_all_stable_fields() {
    let root = temp_dir("item_signatures_rehydrate");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
    let source = crate::source_content_fingerprint("item signature fixture");
    let dependency_source = crate::source_content_fingerprint("dependency fixture");
    let program_sources = crate::frontend_program_source_fingerprint([
        (&module, source, 512),
        (&dependency, dependency_source, 128),
    ]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let key = crate::FrontendSignatureItemSignaturesCacheKey::new(
        namespace,
        &module,
        SignatureItemSet::Types,
        program_sources,
    );

    let mut old_ids = ModuleIdAllocator::new();
    let old_module = old_ids.allocate();
    let old_dependency = old_ids.allocate();
    let old_store = TypeStore::new();
    let append = old_store.append_for_module(old_module);
    let old_symbols = SymbolTable::new();
    let symbol = |text| old_symbols.intern(text).expect("intern fixture symbol");
    let value_name = symbol("value");
    let item_name = symbol("Item");
    let generic_name = symbol("T");
    let primitive = append.intern(TyKind::Primitive(PrimitiveTy::I32));
    let generic = append.intern(TyKind::GenericParam(generic_name));
    let nominal = append.intern(TyKind::Nominal {
        def_id: GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(70),
        },
        args: vec![generic],
        const_args: Vec::new(),
    });
    let trait_ty = append.intern(TyKind::BuiltinTrait {
        trait_id: BuiltinTrait::Iterator,
        args: vec![generic],
    });
    let span = nia_span::Span::new(1, 8);
    let where_predicates = vec![item_signatures::WherePredicateSignature {
        ty: generic,
        bounds: vec![item_signatures::WhereBoundSignature {
            trait_ty,
            associated_type_bindings: vec![item_signatures::AssociatedTypeBindingSignature {
                name: item_name,
                ty: primitive,
                span,
            }],
            span,
        }],
        span,
    }];
    let function = item_signatures::FunctionSignature {
        name: old_symbols.intern("transform").expect("intern function"),
        generics: vec![generic_name],
        generic_params: vec![item_signatures::GenericParamSignature {
            name: generic_name,
            kind: item_signatures::GenericParamSignatureKind::Const { ty: primitive },
        }],
        where_predicates: where_predicates.clone(),
        params: vec![item_signatures::ParamSignature {
            name: Some(value_name),
            receiver: Some(ReceiverKind::RefReadOnly),
            ty: nominal,
            span,
        }],
        return_type: primitive,
        is_extern: false,
        is_const: true,
        is_variadic: false,
        attributes: vec![
            item_signatures::FunctionAttribute::Builtin(BuiltinFunction::SizeOf),
            item_signatures::FunctionAttribute::TrackCaller,
        ],
        has_body: true,
        span,
    };
    let field = item_signatures::FieldSignature {
        def_id: DefId(11),
        name: value_name,
        ty: generic,
        span,
    };
    let signatures = ItemSignatures {
        functions: HashMap::from([(DefId(1), function.clone())]),
        structs: HashMap::from([(
            DefId(2),
            item_signatures::StructSignature {
                generics: vec![generic_name],
                generic_params: vec![item_signatures::GenericParamSignature {
                    name: generic_name,
                    kind: item_signatures::GenericParamSignatureKind::Type,
                }],
                where_predicates: where_predicates.clone(),
                fields: vec![field.clone()],
                is_tuple: true,
                is_extern: false,
                span,
            },
        )]),
        unions: HashMap::from([(
            DefId(3),
            item_signatures::UnionSignature {
                generics: vec![generic_name],
                generic_params: vec![item_signatures::GenericParamSignature {
                    name: generic_name,
                    kind: item_signatures::GenericParamSignatureKind::Const { ty: primitive },
                }],
                where_predicates: Vec::new(),
                fields: vec![field],
                is_extern: true,
                span,
            },
        )]),
        traits: HashMap::from([(
            DefId(4),
            item_signatures::TraitSignature {
                generics: vec![generic_name],
                generic_params: vec![item_signatures::GenericParamSignature {
                    name: generic_name,
                    kind: item_signatures::GenericParamSignatureKind::Const { ty: primitive },
                }],
                where_predicates: where_predicates.clone(),
                supertraits: vec![item_signatures::TraitSupertraitSignature {
                    ty: trait_ty,
                    associated_type_bindings: vec![
                        item_signatures::AssociatedTypeBindingSignature {
                            name: item_name,
                            ty: primitive,
                            span,
                        },
                    ],
                    span,
                }],
                associated_types: vec![item_signatures::TraitAssociatedTypeSignature {
                    def_id: DefId(41),
                    name: item_name,
                    span,
                }],
                associated_values: vec![item_signatures::TraitAssociatedValueSignature {
                    def_id: DefId(42),
                    name: value_name,
                    ty: primitive,
                    span,
                }],
                methods: vec![item_signatures::TraitMethodSignature {
                    def_id: DefId(43),
                    name: symbol("next"),
                    signature: function,
                    has_default: true,
                    span,
                }],
                builtin: Some(BuiltinTrait::Iterator),
                span,
            },
        )]),
        trait_impls: vec![item_signatures::TraitImplSignature {
            impl_id: TraitImplId(51),
            builtin: Some("iterator".to_string()),
            generics: vec![generic_name],
            generic_params: vec![item_signatures::GenericParamSignature {
                name: generic_name,
                kind: item_signatures::GenericParamSignatureKind::Const { ty: primitive },
            }],
            target_ty: nominal,
            trait_ty: Some(trait_ty),
            trait_span: Some(span),
            where_predicates,
            associated_types: vec![item_signatures::TraitImplAssociatedTypeSignature {
                name: item_name,
                ty: primitive,
                span,
            }],
            associated_values: vec![item_signatures::TraitImplAssociatedValueSignature {
                def_id: DefId(52),
                name: value_name,
                visibility: Visibility::PublicPkg,
                span,
            }],
            methods: vec![item_signatures::TraitImplMethodSignature {
                def_id: DefId(53),
                name: symbol("next"),
                visibility: Visibility::Public,
                span,
            }],
            span,
        }],
        enums: HashMap::from([(
            DefId(5),
            item_signatures::EnumSignature {
                backing_type: primitive,
                is_open: true,
                variants: vec![item_signatures::EnumVariantSignature {
                    def_id: DefId(54),
                    name: symbol("First"),
                    payload: item_signatures::EnumVariantPayloadSignature::Unit,
                    span,
                }],
                span,
            },
        )]),
        type_aliases: HashMap::from([(
            DefId(6),
            item_signatures::TypeAliasSignature {
                generics: vec![generic_name],
                generic_params: vec![item_signatures::GenericParamSignature {
                    name: generic_name,
                    kind: item_signatures::GenericParamSignatureKind::Type,
                }],
                target: nominal,
                span,
            },
        )]),
        globals: HashMap::from([(
            DefId(7),
            item_signatures::GlobalSignature {
                explicit_type: Some(primitive),
                is_mutable: true,
                is_extern: false,
                span,
            },
        )]),
        consts: HashMap::from([(
            DefId(8),
            item_signatures::ConstSignature {
                explicit_type: Some(primitive),
                builtin: Some(BuiltinConstValue::TargetPointerWidth),
                span,
            },
        )]),
        diagnostics: Vec::new(),
    };
    let old_paths = HashMap::from([
        (old_module, "src/main.nia".to_string()),
        (old_dependency, "src/dep.nia".to_string()),
    ]);
    let old_payload = encode_item_signatures(&signatures, &old_paths, &old_symbols, &old_store)
        .expect("encode old signatures");
    cache
        .publish_item_signatures(
            SignatureItemSignaturesIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_len: 512,
            },
            &signatures,
            &old_paths,
            &old_symbols,
            &old_store,
            false,
        )
        .expect("publish signatures");

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
        .load_item_signatures(
            SignatureItemSignaturesIdentity {
                key,
                namespace,
                module: &module,
                set: SignatureItemSet::Types,
                program_sources,
                source_len: 512,
            },
            &modules,
            &new_symbols,
            &new_store,
        )
        .expect("load signatures");
    let SignatureItemSignaturesLookup::Hit(loaded) = loaded else {
        panic!("expected signatures cache hit");
    };
    assert!(
        loaded
            .structs
            .get(&DefId(2))
            .is_some_and(|signature| signature.is_tuple)
    );
    assert!(
        loaded
            .type_roots()
            .iter()
            .all(|ty| ty.store_id == new_store.id())
    );
    let new_paths = HashMap::from([
        (new_module, "src/main.nia".to_string()),
        (new_dependency, "src/dep.nia".to_string()),
    ]);
    let new_payload = encode_item_signatures(&loaded, &new_paths, &new_symbols, &new_store)
        .expect("encode rehydrated signatures");
    assert_eq!(old_payload, new_payload);

    let path = cache.item_signatures_path(key);
    let mut corrupt = fs::read(&path).expect("read signatures entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt signatures entry");
    assert_eq!(
        cache
            .load_item_signatures(
                SignatureItemSignaturesIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load corrupt signatures"),
        SignatureItemSignaturesLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn item_signature_decoder_rejects_duplicate_field_names() {
    let symbols = SymbolTable::new();
    let store = TypeStore::new();
    let module = ModuleIdAllocator::new().allocate();
    let ty = store
        .append_for_module(module)
        .intern(TyKind::Primitive(PrimitiveTy::I32));
    let mut encoded = Vec::new();
    write_u64(&mut encoded, 2);
    for def_id in [1_u64, 2] {
        write_u64(&mut encoded, def_id);
        write_string(&mut encoded, "value");
        write_type_index(&mut encoded, 0);
        write_span(&mut encoded, nia_span::Span::new(0, 1));
    }

    assert!(read_fields(&mut Cursor::new(encoded.as_slice()), &[ty], &symbols, 1,).is_none());
}

#[test]
fn item_signature_decoder_rejects_duplicate_generic_names() {
    let symbols = SymbolTable::new();
    let mut encoded = Vec::new();
    write_u64(&mut encoded, 2);
    for _ in 0..2 {
        write_string(&mut encoded, "T");
        encoded.push(0);
    }

    assert!(read_generic_params(&mut Cursor::new(encoded.as_slice()), &[], &symbols,).is_none());
}
