// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{InternedTyId, ModuleId};
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ConstGenericArg, TyKind};

use crate::{
    BackendLowerModuleInput, BackendLowerShared, TypeInstantiationKey, TypeSubstitutionId,
    TypeSubstitutionKey,
};

pub(crate) struct BackendTypeContext<'input, 'shared> {
    input: &'input BackendLowerModuleInput<'input>,
    shared: &'shared BackendLowerShared<'input>,
    pub(crate) interner: nia_ty::TypeStoreModuleCheckout,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    self_substitutions: Vec<Option<InternedTyId>>,
    type_substitutions: Vec<SymbolMap<InternedTyId>>,
    const_substitutions: Vec<SymbolMap<ConstGenericArg>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
}

impl<'input, 'shared> BackendTypeContext<'input, 'shared> {
    pub(crate) fn new(
        input: &'input BackendLowerModuleInput<'input>,
        shared: &'shared BackendLowerShared<'input>,
        interner: nia_ty::TypeStoreModuleCheckout,
    ) -> Self {
        Self {
            input,
            shared,
            interner,
            type_instantiations: HashMap::new(),
            self_substitutions: Vec::new(),
            type_substitutions: Vec::new(),
            const_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
        }
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if let Some(kind) = self.interner.get(ty) {
            return Some(kind);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && let Some(kind) = extension_interner.get(ty)
        {
            return Some(kind);
        }
        self.input_interner_for_type(ty)
            .and_then(|interner| interner.get(ty))
    }

    pub(crate) fn active_interner_for_type(&self, ty: InternedTyId) -> &nia_ty::TyInterner {
        if self.interner.get(ty).is_some() {
            return &self.interner;
        }
        if let Some(extension_interner) = self.input.extension_interner
            && extension_interner.get(ty).is_some()
        {
            return extension_interner;
        }
        let active = self.input_interner_for_type(ty).unwrap_or_else(|| {
            panic!("Nia ICE: backend type {ty:?} is missing from all active input type views")
        });
        require_type_in_interner(active, ty, "input");
        active
    }

    pub(crate) fn active_ty_kind(&self, ty: InternedTyId) -> &TyKind {
        self.active_interner_for_type(ty)
            .get(ty)
            .unwrap_or_else(|| panic!("Nia ICE: backend type {:?} is missing", ty))
    }

    fn input_interner_for_type(&self, ty: InternedTyId) -> Option<&nia_ty::TyInterner> {
        self.shared
            .input_type_interners
            .iter()
            .filter(|(_, interner)| interner.get(ty).is_some())
            .min_by_key(|(module_id, _)| *module_id)
            .map(|(_, interner)| *interner)
    }

    pub(crate) fn type_interner_for_module(
        &self,
        module_id: ModuleId,
    ) -> Option<&'input nia_ty::TyInterner> {
        self.input.program_type_interners.get(&module_id)
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.input.type_normalization.normalize(ty);
        if let Some(layout) = self.input.layouts.types.get(&ty).cloned() {
            return Some(layout);
        }
        let Some(TyKind::Nominal { def_id, args, .. }) = self.ty_kind(ty) else {
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
        let Some(TyKind::Nominal { def_id, args, .. }) = self.ty_kind(ty) else {
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
        self_arg: Option<InternedTyId>,
        substitutions: Vec<(SymbolId, InternedTyId)>,
        const_substitutions: Vec<(SymbolId, ConstGenericArg)>,
    ) -> TypeSubstitutionId {
        let key = TypeSubstitutionKey {
            self_arg,
            substitutions,
            const_substitutions,
        };
        if let Some(id) = self.type_substitution_ids.get(&key) {
            return *id;
        }
        let id = TypeSubstitutionId(self.type_substitutions.len());
        self.self_substitutions.push(key.self_arg);
        self.type_substitutions
            .push(key.substitutions.iter().cloned().collect());
        self.const_substitutions
            .push(key.const_substitutions.iter().cloned().collect());
        self.type_substitution_ids.insert(key, id);
        id
    }

    pub(crate) fn type_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        self.type_substitutions
            .get(substitutions.0)?
            .get(name)
            .copied()
    }

    pub(crate) fn self_substitution(
        &self,
        substitutions: TypeSubstitutionId,
    ) -> Option<InternedTyId> {
        self.self_substitutions
            .get(substitutions.0)
            .copied()
            .flatten()
    }

    pub(crate) fn type_substitutions(
        &self,
        substitutions: TypeSubstitutionId,
    ) -> Option<&SymbolMap<InternedTyId>> {
        self.type_substitutions.get(substitutions.0)
    }

    pub(crate) fn const_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &SymbolId,
    ) -> Option<ConstGenericArg> {
        self.const_substitutions
            .get(substitutions.0)?
            .get(name)
            .cloned()
    }
}

fn require_type_in_interner(interner: &nia_ty::TyInterner, ty: InternedTyId, source: &str) {
    if interner.get(ty).is_none() {
        panic!(
            "Nia ICE: backend type {:?} is not present in {source} interner {:?}",
            ty,
            interner.interner_id()
        );
    }
}
