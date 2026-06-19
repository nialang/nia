// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{InternedTyId, ModuleId};
use nia_ty::TyKind;

use crate::{
    BackendLowerModuleInput, BackendLowerShared, TypeInstantiationKey, TypeSubstitutionId,
    TypeSubstitutionKey, insert_known_type_interner,
};

pub(crate) struct BackendTypeContext<'input, 'shared> {
    input: &'input BackendLowerModuleInput<'input>,
    shared: &'shared BackendLowerShared,
    pub(crate) interner: nia_ty::TyInterner,
    dynamic_type_interners: HashMap<ModuleId, Vec<nia_ty::TyInterner>>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<HashMap<String, InternedTyId>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
}

impl<'input, 'shared> BackendTypeContext<'input, 'shared> {
    pub(crate) fn new(
        input: &'input BackendLowerModuleInput<'input>,
        shared: &'shared BackendLowerShared,
    ) -> Self {
        Self {
            input,
            shared,
            interner: input.function_interner.clone(),
            dynamic_type_interners: HashMap::new(),
            type_instantiations: HashMap::new(),
            type_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
        }
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if ty.interner_id == self.interner.interner_id() {
            return self.interner.get(ty);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.interner_id == extension_interner.interner_id()
        {
            return extension_interner.get(ty);
        }
        self.known_interner_containing_ty(ty)
            .and_then(|interner| interner.get(ty))
    }

    pub(crate) fn known_interner_containing_ty(
        &self,
        ty: InternedTyId,
    ) -> Option<&nia_ty::TyInterner> {
        let mut error_candidate = None;
        let dynamic_interners = self
            .dynamic_type_interners
            .get(&ty.interner_id)
            .into_iter()
            .flat_map(|interners| interners.iter().rev());
        let shared_interners = self
            .shared
            .known_type_interners
            .get(&ty.interner_id)
            .into_iter()
            .flat_map(|interners| interners.iter().rev());
        for interner in dynamic_interners.chain(shared_interners) {
            match interner.get(ty) {
                Some(TyKind::Error) => {
                    error_candidate.get_or_insert(interner);
                }
                Some(_) => return Some(interner),
                None => {}
            }
        }
        error_candidate
    }

    pub(crate) fn remember_interner(&mut self, interner: &nia_ty::TyInterner) {
        insert_known_type_interner(&mut self.dynamic_type_interners, interner);
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.input.type_normalization.normalize(ty);
        if let Some(layout) = self.input.layouts.types.get(&ty).cloned() {
            return Some(layout);
        }
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input.layouts.nominal_type_layout(*def_id, args)
    }

    pub(crate) fn field_offset(
        &self,
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    ) -> Option<u64> {
        let ty = self.input.type_normalization.normalize(ty);
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input.layouts.field_offset(*def_id, args, field)
    }

    pub(crate) fn type_instantiation(&self, key: &TypeInstantiationKey) -> Option<InternedTyId> {
        self.type_instantiations.get(key).copied()
    }

    pub(crate) fn cache_type_instantiation(
        &mut self,
        key: TypeInstantiationKey,
        instantiated: InternedTyId,
    ) -> InternedTyId {
        self.type_instantiations.insert(key, instantiated);
        instantiated
    }

    pub(crate) fn intern_type_substitutions(
        &mut self,
        substitutions: Vec<(String, InternedTyId)>,
    ) -> TypeSubstitutionId {
        let key = TypeSubstitutionKey { substitutions };
        if let Some(id) = self.type_substitution_ids.get(&key) {
            return *id;
        }
        let id = TypeSubstitutionId(self.type_substitutions.len());
        self.type_substitutions
            .push(key.substitutions.iter().cloned().collect());
        self.type_substitution_ids.insert(key, id);
        id
    }

    pub(crate) fn type_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &str,
    ) -> Option<InternedTyId> {
        self.type_substitutions
            .get(substitutions.0)?
            .get(name)
            .copied()
    }

    pub(crate) fn type_substitutions(
        &self,
        substitutions: TypeSubstitutionId,
    ) -> Option<&HashMap<String, InternedTyId>> {
        self.type_substitutions.get(substitutions.0)
    }
}
