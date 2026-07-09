// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct BodyProgramSignatureLookup<'a> {
    pub(super) functions: &'a dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
    pub(super) fallback: ProgramSignatureResolvers<'a>,
    pub(super) maps: Option<ProgramNonFunctionSignatureMaps<'a>>,
}

impl nia_program_signatures::ProgramSignatureLookup for BodyProgramSignatureLookup<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        (self.functions)(def_id)
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.maps
            .and_then(|maps| maps.global(def_id))
            .or_else(|| self.fallback.global(def_id))
    }

    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        self.maps
            .and_then(|maps| maps.comptime(def_id))
            .or_else(|| self.fallback.comptime(def_id))
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.maps
            .and_then(|maps| maps.struct_(def_id))
            .or_else(|| self.fallback.struct_(def_id))
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.maps
            .and_then(|maps| maps.union(def_id))
            .or_else(|| self.fallback.union(def_id))
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.maps
            .and_then(|maps| maps.enum_(def_id))
            .or_else(|| self.fallback.enum_(def_id))
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.maps
            .and_then(|maps| maps.trait_(def_id))
            .or_else(|| self.fallback.trait_(def_id))
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.maps
            .and_then(|maps| maps.type_alias(def_id))
            .or_else(|| self.fallback.type_alias(def_id))
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        if let Some(maps) = self.maps {
            return maps.trait_ids_with_method_named(name);
        }
        self.fallback.trait_ids_with_method_named(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.maps
            .and_then(|maps| maps.trait_owning_method(method_id))
            .or_else(|| self.fallback.trait_owning_method(method_id))
    }
}
