// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct TestBackendProgramFacts<'a> {
    source_identities: HashMap<ModuleId, nia_source::SourceIdentity>,
    symbol_package_identities: HashMap<ModuleId, String>,
    const_array_lengths: HashMap<ModuleId, &'a HashMap<GlobalConstExprId, u64>>,
    function_body_ids: Vec<GlobalDefId>,
    function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
    static_init_ids: Vec<GlobalDefId>,
    static_inits: HashMap<GlobalDefId, &'a nia_static_ir::StaticInit>,
    extension_methods: nia_defs::ExtensionMethods,
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
    trait_impl_index: nia_item_signatures::ProgramTraitImplIndex,
}

impl<'a> TestBackendProgramFacts<'a> {
    pub(super) fn new(
        module_id: ModuleId,
        const_array_lengths: HashMap<ModuleId, &'a HashMap<GlobalConstExprId, u64>>,
        function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
        static_inits: HashMap<GlobalDefId, &'a nia_static_ir::StaticInit>,
    ) -> Self {
        let mut function_body_ids = function_bodies.keys().copied().collect::<Vec<_>>();
        function_body_ids.sort_unstable();
        let mut static_init_ids = static_inits.keys().copied().collect::<Vec<_>>();
        static_init_ids.sort_unstable();
        Self {
            source_identities: HashMap::from([(
                module_id,
                nia_source::SourceIdentity::new("main"),
            )]),
            symbol_package_identities: HashMap::from([(module_id, "test/main@0".into())]),
            const_array_lengths,
            function_body_ids,
            function_bodies,
            static_init_ids,
            static_inits,
            extension_methods: nia_defs::ExtensionMethods::default(),
            functions: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            traits: HashMap::new(),
            type_aliases: HashMap::new(),
            trait_impls: Vec::new(),
            trait_impl_index: nia_item_signatures::ProgramTraitImplIndex::default(),
        }
    }

    pub(super) fn set_signature_facts(
        &mut self,
        module_id: ModuleId,
        type_store: &nia_ty::TypeStore,
        defs: &DefCollection,
        lowering: &TypeLowering,
        signatures: &ItemSignatures,
        normalization: &nia_type_normalize::TypeNormalization,
    ) {
        let signature_input = nia_program_signatures::ModuleSignatureInput {
            module_id,
            type_store,
            signatures,
        };
        let facts = nia_program_signatures::collect_module_program_signature_facts(signature_input);
        let extension_input = nia_program_signatures::ExtensionModuleInput {
            module_id,
            type_store,
            defs,
            lowering,
            signatures,
            function_signatures: signatures,
            type_signatures: signatures,
            normalization,
        };
        self.trait_impls =
            nia_program_signatures::collect_valid_program_trait_impls(&[extension_input]);
        self.trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new_with_type_store(
            &self.trait_impls,
            type_store,
        );
        self.extension_methods = nia_program_signatures::collect_extension_method_index_for_module(
            &nia_program_signatures::ExtensionMethodIndexModuleInput {
                module_id,
                type_store,
                defs,
                lowering,
                signatures,
                normalization,
            },
            &facts.trait_defs,
        );
        self.functions = facts.functions;
        self.structs = facts.structs;
        self.unions = facts.unions;
        self.enums = facts.enums;
        self.traits = facts.traits;
        self.type_aliases = facts.type_aliases;
    }
}

impl BackendProgramFacts for TestBackendProgramFacts<'_> {
    fn source_identities(&self) -> &HashMap<ModuleId, nia_source::SourceIdentity> {
        &self.source_identities
    }

    fn symbol_package_identities(&self) -> &HashMap<ModuleId, String> {
        &self.symbol_package_identities
    }

    fn const_array_lengths(&self, module_id: ModuleId) -> Option<&HashMap<GlobalConstExprId, u64>> {
        self.const_array_lengths.get(&module_id).copied()
    }

    fn function_body_ids(&self) -> &[GlobalDefId] {
        &self.function_body_ids
    }

    fn function_body(&self, def_id: GlobalDefId) -> Option<&nia_function_ir::FunctionBody> {
        self.function_bodies.get(&def_id).copied()
    }

    fn static_init_ids(&self) -> &[GlobalDefId] {
        &self.static_init_ids
    }

    fn static_init(&self, def_id: GlobalDefId) -> Option<&nia_static_ir::StaticInit> {
        self.static_inits.get(&def_id).copied()
    }

    fn extension_methods(&self) -> &nia_defs::ExtensionMethods {
        &self.extension_methods
    }

    fn extensions(&self, _module_id: ModuleId) -> Option<&VisibleExtensionMethods> {
        None
    }

    fn defs(&self, _module_id: ModuleId) -> Option<&DefCollection> {
        None
    }

    fn normalized_type(&self, _ty: InternedTyId) -> Option<InternedTyId> {
        None
    }

    fn normalized_type_from_module(
        &self,
        _module_id: ModuleId,
        _ty: InternedTyId,
    ) -> Option<InternedTyId> {
        None
    }

    fn functions(&self) -> &HashMap<GlobalDefId, ProgramFunctionSignature> {
        &self.functions
    }

    fn structs(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature> {
        &self.structs
    }

    fn unions(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature> {
        &self.unions
    }

    fn enums(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature> {
        &self.enums
    }

    fn traits(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature> {
        &self.traits
    }

    fn type_aliases(
        &self,
    ) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature> {
        &self.type_aliases
    }

    fn trait_impls(&self) -> &[nia_item_signatures::ProgramTraitImplSignature] {
        &self.trait_impls
    }

    fn trait_impl_index(&self) -> &nia_item_signatures::ProgramTraitImplIndex {
        &self.trait_impl_index
    }
}
