// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct EmptyBodyProgramSignatures {
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    globals: HashMap<GlobalDefId, nia_item_signatures::ProgramGlobalSignature>,
    consts: HashMap<GlobalDefId, nia_item_signatures::ProgramConstSignature>,
    structs: HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature>,
    unions: HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature>,
    enums: HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature>,
    traits: HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature>,
    type_aliases: HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    trait_impls: Vec<nia_item_signatures::ProgramTraitImplSignature>,
}

impl EmptyBodyProgramSignatures {
    pub(super) fn new() -> Self {
        Self {
            functions: HashMap::new(),
            globals: HashMap::new(),
            consts: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            traits: HashMap::new(),
            type_aliases: HashMap::new(),
            trait_impls: Vec::new(),
        }
    }

    pub(super) fn context(&self) -> ProgramSignatureContext<'_> {
        ProgramSignatureContext {
            lookup: self,
            trait_impls: &self.trait_impls,
            trait_impl_index: None,
        }
    }
}

impl ProgramSignatureLookup for EmptyBodyProgramSignatures {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        self.functions.get(&def_id).cloned()
    }

    fn global(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    fn const_eval(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramConstSignature> {
        self.consts.get(&def_id).cloned()
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    fn union(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<nia_item_signatures::ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    fn type_alias(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    fn trait_ids_with_method_named(&self, name: &nia_symbol::SymbolId) -> Vec<GlobalDefId> {
        self.traits
            .iter()
            .filter_map(|(trait_id, signature)| {
                signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| &method.name == name)
                    .then_some(*trait_id)
            })
            .collect()
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, nia_item_signatures::ProgramTraitSignature)> {
        self.traits.iter().find_map(|(trait_id, signature)| {
            signature
                .signature
                .methods
                .iter()
                .any(|method| {
                    GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    } == method_id
                })
                .then(|| (*trait_id, signature.clone()))
        })
    }
}

pub(super) fn semantic_use_table(
    module_id: ModuleId,
    values: &nia_value_resolve::ValueResolution,
    locals: &nia_local_resolve::LocalResolution,
    type_lowering: &TypeLowering,
    active_item_tree: &ActiveModuleItemTree,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder.extend_node_type_uses(
        type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree),
    );
    builder.finish()
}
