// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{InternedTyId, ModuleId, TyInternerId};
use nia_ty::TyKind;

use crate::{
    BackendLowerModuleInput, BackendLowerShared, TypeInstantiationKey, TypeSubstitutionId,
    TypeSubstitutionKey, insert_known_type_interner,
};

pub(crate) struct BackendTypeContext<'input, 'shared> {
    input: &'input BackendLowerModuleInput<'input>,
    shared: &'shared BackendLowerShared,
    pub(crate) interner: nia_ty::TyInterner,
    dynamic_type_interners: HashMap<TyInternerId, nia_ty::TyInterner>,
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
        Some(self.active_ty_kind(ty))
    }

    pub(crate) fn active_interner_for_type(&self, ty: InternedTyId) -> &nia_ty::TyInterner {
        if ty.interner_id == self.interner.interner_id() {
            if self.interner.get(ty).is_none() {
                panic!(
                    "Nia ICE: backend type {:?} is not present in current interner {:?}",
                    ty,
                    self.interner.interner_id()
                );
            }
            return &self.interner;
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.interner_id == extension_interner.interner_id()
        {
            if extension_interner.get(ty).is_none() {
                panic!(
                    "Nia ICE: backend type {:?} is not present in extension interner {:?}",
                    ty,
                    extension_interner.interner_id()
                );
            }
            return extension_interner;
        }
        let shared = self.shared.known_type_interners.get(&ty.interner_id);
        let active = if let Some(dynamic) = self.dynamic_type_interners.get(&ty.interner_id) {
            if let Some(shared) = shared
                && !shared.is_prefix_of(dynamic)
            {
                panic!(
                    "Nia ICE: backend dynamic type interner {:?} diverged from shared snapshot",
                    ty.interner_id
                );
            }
            dynamic
        } else {
            shared.unwrap_or_else(|| {
                panic!(
                    "Nia ICE: missing backend type interner {:?} for type {:?}",
                    ty.interner_id, ty
                )
            })
        };
        if active.get(ty).is_none() {
            panic!(
                "Nia ICE: backend type {:?} is not present in active interner {:?}",
                ty,
                active.interner_id()
            );
        }
        active
    }

    pub(crate) fn active_ty_kind(&self, ty: InternedTyId) -> &TyKind {
        self.active_interner_for_type(ty)
            .get(ty)
            .unwrap_or_else(|| panic!("Nia ICE: backend type {:?} is missing", ty))
    }

    pub(crate) fn remember_interner(&mut self, interner: &nia_ty::TyInterner) {
        insert_known_type_interner(&mut self.dynamic_type_interners, interner);
    }

    pub(crate) fn function_body_interner(
        &self,
        module_id: ModuleId,
    ) -> Option<&'input nia_ty::TyInterner> {
        if module_id == self.input.module_id {
            return Some(&self.input.body_ir.interner);
        }
        self.input
            .program_function_body_interners
            .for_module(module_id)
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
