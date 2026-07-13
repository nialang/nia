// SPDX-License-Identifier: GPL-3.0-or-later
mod analysis;

use std::collections::HashMap;

use nia_ids::GlobalDefId;
use nia_item_signatures::{
    ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramStructSignature, ProgramTraitImplIndex,
    ProgramTraitImplSignature, ProgramTraitSignature, ProgramTypeAliasSignature,
    ProgramUnionSignature,
};
use nia_symbol::{SymbolId, SymbolMap};

pub use analysis::*;

pub trait ProgramSignatureLookup {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature>;
    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature>;
    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature>;
    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature>;
    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature>;
    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature>;
    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature>;
    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature>;
    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId>;
    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)>;

    fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.function(def_id).is_some()
    }

    fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.global(def_id).is_some()
    }

    fn has_comptime(&self, def_id: GlobalDefId) -> bool {
        self.comptime(def_id).is_some()
    }

    fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.struct_(def_id).is_some()
    }

    fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.union(def_id).is_some()
    }

    fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enum_(def_id).is_some()
    }

    fn has_trait(&self, def_id: GlobalDefId) -> bool {
        self.trait_(def_id).is_some()
    }

    fn has_type_alias(&self, def_id: GlobalDefId) -> bool {
        self.type_alias(def_id).is_some()
    }
}

#[derive(Clone, Copy)]
pub struct ProgramSignatureContext<'a> {
    pub lookup: &'a dyn ProgramSignatureLookup,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub trait_impl_index: Option<&'a ProgramTraitImplIndex>,
}

impl<'a> ProgramSignatureContext<'a> {
    pub fn new(
        lookup: &'a dyn ProgramSignatureLookup,
        trait_impls: &'a [ProgramTraitImplSignature],
    ) -> Self {
        Self {
            lookup,
            trait_impls,
            trait_impl_index: None,
        }
    }

    pub fn new_indexed(
        lookup: &'a dyn ProgramSignatureLookup,
        trait_impls: &'a [ProgramTraitImplSignature],
        trait_impl_index: &'a ProgramTraitImplIndex,
    ) -> Self {
        Self {
            lookup,
            trait_impls,
            trait_impl_index: Some(trait_impl_index),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyProgramSignatureLookup;

pub static EMPTY_PROGRAM_SIGNATURE_LOOKUP: EmptyProgramSignatureLookup =
    EmptyProgramSignatureLookup;

impl ProgramSignatureLookup for EmptyProgramSignatureLookup {
    fn function(&self, _def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        None
    }

    fn global(&self, _def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        None
    }

    fn comptime(&self, _def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        None
    }

    fn struct_(&self, _def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        None
    }

    fn union(&self, _def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        None
    }

    fn enum_(&self, _def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        None
    }

    fn trait_(&self, _def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        None
    }

    fn type_alias(&self, _def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        None
    }

    fn trait_ids_with_method_named(&self, _name: &SymbolId) -> Vec<GlobalDefId> {
        Vec::new()
    }

    fn trait_owning_method(
        &self,
        _method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        None
    }
}

impl ProgramSignatureContext<'static> {
    pub fn empty() -> Self {
        Self {
            lookup: &EMPTY_PROGRAM_SIGNATURE_LOOKUP,
            trait_impls: &[],
            trait_impl_index: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramSignatureMaps<'a> {
    pub functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub comptimes: &'a HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub trait_method_index: &'a ProgramTraitMethodIndex,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramNonFunctionSignatureMaps<'a> {
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub comptimes: &'a HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub trait_method_index: &'a ProgramTraitMethodIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgramTraitMethodIndex {
    trait_ids_by_method_name: SymbolMap<Vec<GlobalDefId>>,
    trait_id_by_method_id: HashMap<GlobalDefId, GlobalDefId>,
}

impl ProgramTraitMethodIndex {
    pub fn from_traits(
        traits: &HashMap<GlobalDefId, ProgramTraitSignature>,
    ) -> ProgramTraitMethodIndex {
        Self::from_trait_signatures(
            traits
                .iter()
                .map(|(trait_id, signature)| (*trait_id, signature)),
        )
    }

    pub fn from_trait_signatures<'a>(
        traits: impl IntoIterator<Item = (GlobalDefId, &'a ProgramTraitSignature)>,
    ) -> ProgramTraitMethodIndex {
        let mut trait_ids_by_method_name: SymbolMap<Vec<GlobalDefId>> = SymbolMap::default();
        let mut trait_id_by_method_id = HashMap::new();
        for (trait_id, signature) in traits {
            for method in &signature.signature.methods {
                trait_ids_by_method_name
                    .entry(method.name)
                    .or_default()
                    .push(trait_id);
                trait_id_by_method_id.insert(
                    GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    },
                    trait_id,
                );
            }
        }
        for trait_ids in trait_ids_by_method_name.values_mut() {
            trait_ids.sort();
            trait_ids.dedup();
        }
        ProgramTraitMethodIndex {
            trait_ids_by_method_name,
            trait_id_by_method_id,
        }
    }

    pub fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_ids_by_method_name
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    pub fn trait_owning_method_id(&self, method_id: GlobalDefId) -> Option<GlobalDefId> {
        self.trait_id_by_method_id.get(&method_id).copied()
    }
}

impl ProgramSignatureLookup for ProgramSignatureMaps<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        self.functions.get(&def_id).cloned()
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        self.comptimes.get(&def_id).cloned()
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_method_index.trait_ids_with_method_named(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.trait_method_index
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                self.traits
                    .get(&trait_id)
                    .cloned()
                    .map(|signature| (trait_id, signature))
            })
    }

    fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.functions.contains_key(&def_id)
    }

    fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.globals.contains_key(&def_id)
    }

    fn has_comptime(&self, def_id: GlobalDefId) -> bool {
        self.comptimes.contains_key(&def_id)
    }

    fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.structs.contains_key(&def_id)
    }

    fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.unions.contains_key(&def_id)
    }

    fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enums.contains_key(&def_id)
    }

    fn has_trait(&self, def_id: GlobalDefId) -> bool {
        self.traits.contains_key(&def_id)
    }

    fn has_type_alias(&self, def_id: GlobalDefId) -> bool {
        self.type_aliases.contains_key(&def_id)
    }
}

impl ProgramNonFunctionSignatureMaps<'_> {
    pub fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    pub fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        self.comptimes.get(&def_id).cloned()
    }

    pub fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    pub fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    pub fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    pub fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    pub fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    pub fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_method_index.trait_ids_with_method_named(name)
    }

    pub fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.trait_method_index
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                self.traits
                    .get(&trait_id)
                    .cloned()
                    .map(|signature| (trait_id, signature))
            })
    }
}

#[derive(Clone, Copy)]
pub struct ProgramSignatureResolvers<'a> {
    pub function: &'a dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
    pub global: &'a dyn Fn(GlobalDefId) -> Option<ProgramGlobalSignature>,
    pub comptime: &'a dyn Fn(GlobalDefId) -> Option<ProgramComptimeSignature>,
    pub struct_: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    pub union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    pub enum_: &'a dyn Fn(GlobalDefId) -> Option<ProgramEnumSignature>,
    pub trait_: &'a dyn Fn(GlobalDefId) -> Option<ProgramTraitSignature>,
    pub type_alias: &'a dyn Fn(GlobalDefId) -> Option<ProgramTypeAliasSignature>,
    pub trait_ids_with_method_named: &'a dyn Fn(&SymbolId) -> Vec<GlobalDefId>,
    pub trait_owning_method:
        &'a dyn Fn(GlobalDefId) -> Option<(GlobalDefId, ProgramTraitSignature)>,
}

impl ProgramSignatureLookup for ProgramSignatureResolvers<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        (self.function)(def_id)
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        (self.global)(def_id)
    }

    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        (self.comptime)(def_id)
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        (self.struct_)(def_id)
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        (self.union)(def_id)
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        (self.enum_)(def_id)
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        (self.trait_)(def_id)
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        (self.type_alias)(def_id)
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        (self.trait_ids_with_method_named)(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        (self.trait_owning_method)(method_id)
    }
}
