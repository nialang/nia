// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalDefId, ModuleId};

use crate::{ExtensionTraitMethodCandidate, ExtensionTraitMethodKey, TypeSubstitutionId};

#[derive(Default)]
pub(crate) struct BackendInstantiationContext<'a> {
    pub(crate) extension_trait_method_candidates: Option<(
        ModuleId,
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    )>,
    pub(crate) extension_interner: Option<&'a nia_ty::TyInterner>,
    pub(crate) function: Option<GlobalDefId>,
    pub(crate) instantiation_module_id: Option<ModuleId>,
    pub(crate) body_interner: Option<&'a nia_ty::TyInterner>,
    pub(crate) type_substitutions: Option<TypeSubstitutionId>,
}

pub(crate) struct BackendInstantiationSnapshot<'a> {
    extension_trait_method_candidates: Option<(
        ModuleId,
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    )>,
    extension_interner: Option<&'a nia_ty::TyInterner>,
    function: Option<GlobalDefId>,
    instantiation_module_id: Option<ModuleId>,
    body_interner: Option<&'a nia_ty::TyInterner>,
    type_substitutions: Option<TypeSubstitutionId>,
}

impl<'a> BackendInstantiationContext<'a> {
    pub(crate) fn take_snapshot(&mut self) -> BackendInstantiationSnapshot<'a> {
        BackendInstantiationSnapshot {
            extension_trait_method_candidates: self.extension_trait_method_candidates.take(),
            extension_interner: self.extension_interner.take(),
            function: self.function.take(),
            instantiation_module_id: self.instantiation_module_id.take(),
            body_interner: self.body_interner.take(),
            type_substitutions: self.type_substitutions.take(),
        }
    }

    pub(crate) fn restore(&mut self, snapshot: BackendInstantiationSnapshot<'a>) {
        self.extension_trait_method_candidates = snapshot.extension_trait_method_candidates;
        self.extension_interner = snapshot.extension_interner;
        self.function = snapshot.function;
        self.instantiation_module_id = snapshot.instantiation_module_id;
        self.body_interner = snapshot.body_interner;
        self.type_substitutions = snapshot.type_substitutions;
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
        body_interner: Option<&'a nia_ty::TyInterner>,
        type_substitutions: TypeSubstitutionId,
    ) {
        self.function = Some(function);
        self.instantiation_module_id = Some(instantiation_module_id);
        self.body_interner = body_interner;
        self.type_substitutions = Some(type_substitutions);
    }
}
