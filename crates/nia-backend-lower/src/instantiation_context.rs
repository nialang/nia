// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalDefId, ModuleId};

use crate::TypeSubstitutionId;

#[derive(Default)]
pub(crate) struct BackendInstantiationContext {
    pub(crate) function: Option<GlobalDefId>,
    pub(crate) instantiation_module_id: Option<ModuleId>,
    pub(crate) type_substitutions: Option<TypeSubstitutionId>,
    pub(crate) defer_concrete_trait_diagnostics: bool,
}

pub(crate) struct BackendInstantiationSnapshot {
    function: Option<GlobalDefId>,
    instantiation_module_id: Option<ModuleId>,
    type_substitutions: Option<TypeSubstitutionId>,
    defer_concrete_trait_diagnostics: bool,
}

impl BackendInstantiationContext {
    pub(crate) fn take_snapshot(&mut self) -> BackendInstantiationSnapshot {
        BackendInstantiationSnapshot {
            function: self.function.take(),
            instantiation_module_id: self.instantiation_module_id.take(),
            type_substitutions: self.type_substitutions.take(),
            defer_concrete_trait_diagnostics: self.defer_concrete_trait_diagnostics,
        }
    }

    pub(crate) fn restore(&mut self, snapshot: BackendInstantiationSnapshot) {
        self.function = snapshot.function;
        self.instantiation_module_id = snapshot.instantiation_module_id;
        self.type_substitutions = snapshot.type_substitutions;
        self.defer_concrete_trait_diagnostics = snapshot.defer_concrete_trait_diagnostics;
    }

    pub(crate) fn set_function_scope(
        &mut self,
        function: GlobalDefId,
        type_substitutions: Option<TypeSubstitutionId>,
    ) {
        self.function = Some(function);
        self.type_substitutions = type_substitutions;
    }

    pub(crate) fn set_instance_scope(
        &mut self,
        function: GlobalDefId,
        instantiation_module_id: ModuleId,
        type_substitutions: TypeSubstitutionId,
        defer_concrete_trait_diagnostics: bool,
    ) {
        self.function = Some(function);
        self.instantiation_module_id = Some(instantiation_module_id);
        self.type_substitutions = Some(type_substitutions);
        self.defer_concrete_trait_diagnostics = defer_concrete_trait_diagnostics;
    }
}
