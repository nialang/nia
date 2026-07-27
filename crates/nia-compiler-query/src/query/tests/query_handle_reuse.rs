// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn compiler_fact_batches_reuse_cached_product_handles() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S { value: i32 } extend S { fn get(self) i32 { self.value } }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());
    let signature_set = nia_item_tree::SignatureItemSet::Types;

    let signature = db.expect_get(ModuleProgramSignatureFactsQuery(module_id, signature_set));
    let abi = db.expect_get(ModuleAbiSignatureFactsQuery(module_id));
    let trait_solving = db.expect_get(ExtensionTraitSolvingModuleFactsQuery(module_id));
    let provider = db.expect_get(ExtensionProviderModuleFactsQuery(module_id));
    let nominal = db.expect_get(ExtensionProviderNominalModuleFactsQuery(module_id));
    let visible_extensions: Arc<VisibleExtensionsForModule> =
        db.expect_get(VisibleExtensionsQuery(module_id));
    let visible_trait_impls: Arc<VisibleTraitImplsForModule> =
        db.expect_get(VisibleTraitImplsQuery(module_id));
    let trait_method_index: Arc<nia_program_signatures::ProgramTraitMethodIndex> =
        db.expect_get(ProgramTraitMethodIndexQuery);
    let abi_signatures: Arc<ProgramAbiSignaturesValue> = db.expect_get(ProgramAbiSignaturesQuery);

    let signature_batch = db
        .get_many([ModuleProgramSignatureFactsQuery(module_id, signature_set)])
        .expect("signature batch should succeed");
    let abi_batch = db
        .get_many([ModuleAbiSignatureFactsQuery(module_id)])
        .expect("ABI batch should succeed");
    let trait_solving_batch = db
        .get_many([ExtensionTraitSolvingModuleFactsQuery(module_id)])
        .expect("trait-solving batch should succeed");
    let provider_batch = db
        .get_many([ExtensionProviderModuleFactsQuery(module_id)])
        .expect("provider batch should succeed");
    let nominal_batch = db
        .get_many([ExtensionProviderNominalModuleFactsQuery(module_id)])
        .expect("nominal provider batch should succeed");
    let visible_extensions_batch = db
        .get_many([VisibleExtensionsQuery(module_id)])
        .expect("visible extension batch should succeed");
    let visible_trait_impls_batch = db
        .get_many([VisibleTraitImplsQuery(module_id)])
        .expect("visible trait-impl batch should succeed");
    let trait_method_index_batch = db
        .get_many([ProgramTraitMethodIndexQuery])
        .expect("trait-method index batch should succeed");
    let abi_signatures_batch = db
        .get_many([ProgramAbiSignaturesQuery])
        .expect("program ABI batch should succeed");

    assert!(Arc::ptr_eq(&signature, &signature_batch[0]));
    assert!(Arc::ptr_eq(&abi, &abi_batch[0]));
    assert!(Arc::ptr_eq(&trait_solving, &trait_solving_batch[0]));
    assert!(Arc::ptr_eq(&provider, &provider_batch[0]));
    assert!(Arc::ptr_eq(&nominal, &nominal_batch[0]));
    assert!(Arc::ptr_eq(
        &visible_extensions,
        &visible_extensions_batch[0]
    ));
    assert!(Arc::ptr_eq(
        &visible_trait_impls,
        &visible_trait_impls_batch[0]
    ));
    assert!(Arc::ptr_eq(
        &trait_method_index,
        &trait_method_index_batch[0]
    ));
    assert!(Arc::ptr_eq(&abi_signatures, &abi_signatures_batch[0]));
}

#[test]
fn extension_index_queries_reuse_single_layer_product_handles() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "trait Read { fn read(self) i32; } struct S { value: i32 } extend S { fn get(self) i32 { self.value } }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());
    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let trait_id = nia_ty::TraitId::Source(GlobalDefId {
        module_id,
        def_id: defs.semantic.module_scope.types.get(&sym("Read")).unwrap(),
    });
    let method_id = GlobalDefId {
        module_id,
        def_id: nia_ids::DefId(0),
    };

    let validation: Arc<ExtensionProviderValidationFactsQueryValue> =
        db.expect_get(ExtensionProviderValidationFactsQuery(module_id));
    let discovery: Arc<ExtensionProviderDiscoveryIndexQueryValue> =
        db.expect_get(ExtensionProviderDiscoveryIndexQuery);
    let exposure: Arc<TypeExposureIndex> = db.expect_get(TypeExposureIndexQuery);
    let methods: Arc<ExtensionMethodIndexQueryValue> = db.expect_get(ExtensionMethodIndexQuery);
    let named: Arc<ExtensionMethodsNamedQueryValue> =
        db.expect_get(ExtensionMethodsNamedQuery(sym("get")));
    let method: Arc<ExtensionMethodByIdQueryValue> =
        db.expect_get(ExtensionMethodByIdQuery(method_id));
    let trait_index: Arc<ExtensionTraitSignatureIndex> =
        db.expect_get(ExtensionTraitSignatureIndexQuery);
    let signature_input: Arc<ExtensionSignatureModuleInputQueryValue> =
        db.expect_get(ExtensionSignatureModuleInputQuery(module_id));
    let trait_impls: Arc<ExtensionTraitImplsForTraitQueryValue> =
        db.expect_get(ExtensionTraitImplsForTraitQuery(trait_id));

    let validation_batch = db
        .get_many([ExtensionProviderValidationFactsQuery(module_id)])
        .expect("validation batch should succeed");
    let discovery_batch = db
        .get_many([ExtensionProviderDiscoveryIndexQuery])
        .expect("discovery batch should succeed");
    let exposure_batch = db
        .get_many([TypeExposureIndexQuery])
        .expect("exposure batch should succeed");
    let methods_batch = db
        .get_many([ExtensionMethodIndexQuery])
        .expect("method index batch should succeed");
    let named_batch = db
        .get_many([ExtensionMethodsNamedQuery(sym("get"))])
        .expect("named method batch should succeed");
    let method_batch = db
        .get_many([ExtensionMethodByIdQuery(method_id)])
        .expect("method batch should succeed");
    let trait_index_batch = db
        .get_many([ExtensionTraitSignatureIndexQuery])
        .expect("trait signature batch should succeed");
    let signature_input_batch = db
        .get_many([ExtensionSignatureModuleInputQuery(module_id)])
        .expect("signature input batch should succeed");
    let trait_impls_batch = db
        .get_many([ExtensionTraitImplsForTraitQuery(trait_id)])
        .expect("trait impl batch should succeed");

    assert!(Arc::ptr_eq(&validation, &validation_batch[0]));
    assert!(Arc::ptr_eq(&discovery, &discovery_batch[0]));
    assert!(Arc::ptr_eq(&exposure, &exposure_batch[0]));
    assert!(Arc::ptr_eq(&methods, &methods_batch[0]));
    assert!(Arc::ptr_eq(&named, &named_batch[0]));
    assert!(Arc::ptr_eq(&method, &method_batch[0]));
    assert!(Arc::ptr_eq(&trait_index, &trait_index_batch[0]));
    assert!(Arc::ptr_eq(&signature_input, &signature_input_batch[0]));
    assert!(Arc::ptr_eq(&trait_impls, &trait_impls_batch[0]));
}

#[test]
fn public_surface_queries_reuse_single_layer_product_handles() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let surfaces: Arc<PublicSurfacesQueryValue> = db.expect_get(PublicSurfacesQuery);
    let using_scopes: Arc<PublicUsingScopesQueryValue> = db.expect_get(PublicUsingScopesQuery);
    let module_using_scope: Arc<ModuleUsingScope> = db.expect_get(ModuleUsingScopeQuery(module_id));

    let surfaces_batch = db
        .get_many([PublicSurfacesQuery])
        .expect("public surfaces batch should succeed");
    let using_scopes_batch = db
        .get_many([PublicUsingScopesQuery])
        .expect("public using scopes batch should succeed");
    let module_using_scope_batch = db
        .get_many([ModuleUsingScopeQuery(module_id)])
        .expect("module using scope batch should succeed");

    assert!(Arc::ptr_eq(&surfaces, &surfaces_batch[0]));
    assert!(Arc::ptr_eq(&using_scopes, &using_scopes_batch[0]));
    assert!(Arc::ptr_eq(
        &module_using_scope,
        &module_using_scope_batch[0]
    ));
}
