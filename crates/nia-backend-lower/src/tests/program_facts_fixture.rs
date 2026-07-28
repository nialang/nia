// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct TestBackendProgramFacts<'a> {
    const_array_lengths: HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
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
        const_array_lengths: HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
        function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
        static_inits: HashMap<GlobalDefId, &'a nia_static_ir::StaticInit>,
    ) -> Self {
        let mut function_body_ids = function_bodies.keys().copied().collect::<Vec<_>>();
        function_body_ids.sort_unstable();
        let mut static_init_ids = static_inits.keys().copied().collect::<Vec<_>>();
        static_init_ids.sort_unstable();
        Self {
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
}

impl BackendProgramFacts for TestBackendProgramFacts<'_> {
    fn const_array_lengths(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_const_check::ConstArrayLengths> {
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
