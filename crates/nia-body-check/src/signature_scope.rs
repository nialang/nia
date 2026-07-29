// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Clone, Copy)]
pub(super) enum ProgramSignatureScope<'a> {
    LocalModule,
    Program(&'a dyn ProgramSignatureLookup),
}

impl<'a> ProgramSignatureScope<'a> {
    pub(super) fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.function(def_id),
        }
    }

    pub(super) fn includes_function(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => true,
            ProgramSignatureScope::Program(program) => program.has_function(def_id),
        }
    }

    pub(super) fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.global(def_id),
        }
    }

    pub(super) fn const_eval(&self, def_id: GlobalDefId) -> Option<ProgramConstSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.const_eval(def_id),
        }
    }

    pub(super) fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.struct_(def_id),
        }
    }

    pub(super) fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.union(def_id),
        }
    }

    pub(super) fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.enum_(def_id),
        }
    }

    pub(super) fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.trait_(def_id),
        }
    }

    pub(super) fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.type_alias(def_id),
        }
    }

    pub(super) fn has_enum(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => false,
            ProgramSignatureScope::Program(program) => program.has_enum(def_id),
        }
    }

    pub(super) fn has_union(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => false,
            ProgramSignatureScope::Program(program) => program.has_union(def_id),
        }
    }

    pub(super) fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        match self {
            ProgramSignatureScope::LocalModule => Vec::new(),
            ProgramSignatureScope::Program(program) => program.trait_ids_with_method_named(name),
        }
    }

    pub(super) fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.trait_owning_method(method_id),
        }
    }
}
