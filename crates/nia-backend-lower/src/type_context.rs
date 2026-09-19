// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ice::Ice;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

use nia_ids::InternedTyId;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ConstGenericArg, TyKind};

use crate::{
    BackendLowerModuleInput, TypeInstantiationKey, TypeSubstitutionId, TypeSubstitutionKey,
};

pub(crate) struct BackendTypeContext<'input> {
    input: &'input BackendLowerModuleInput<'input>,
    type_store: &'input nia_ty::TypeStore,
    pub(crate) append: nia_ty::TypeStoreAppend,
    internal_error: Arc<Mutex<Option<Ice>>>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    self_substitutions: Vec<Option<InternedTyId>>,
    type_substitutions: Vec<SymbolMap<InternedTyId>>,
    const_substitutions: Vec<SymbolMap<ConstGenericArg>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
}

impl<'input> BackendTypeContext<'input> {
    pub(crate) fn new(
        input: &'input BackendLowerModuleInput<'input>,
        type_store: &'input nia_ty::TypeStore,
    ) -> Self {
        Self {
            input,
            type_store,
            append: type_store.append_for_module(input.module_id),
            internal_error: Arc::new(Mutex::new(None)),
            type_instantiations: HashMap::new(),
            self_substitutions: Vec::new(),
            type_substitutions: Vec::new(),
            const_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
        }
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
    }

    pub(crate) fn intern(&self, kind: TyKind) -> InternedTyId {
        match self.append.intern(kind) {
            Ok(ty) => ty,
            Err(error) => {
                let mut slot = self.internal_error.lock();
                if slot.is_none() {
                    *slot = Some(error);
                }
                self.type_store.error()
            }
        }
    }

    pub(crate) fn record_internal(&self, error: Ice) {
        let mut slot = self.internal_error.lock();
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    pub(crate) fn internal_error(&self) -> Option<Ice> {
        self.internal_error.lock().clone()
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.input.type_normalization.normalize(ty);
        if let Some(layout) = self.input.layouts.types.get(&ty).cloned() {
            return Some(layout);
        }
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(ty)
        else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input
            .layouts
            .nominal_type_layout_with_const_args(*def_id, args, const_args)
    }

    pub(crate) fn field_offset(
        &self,
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    ) -> Option<u64> {
        let ty = self.input.type_normalization.normalize(ty);
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(ty)
        else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input
            .layouts
            .field_offset_with_const_args(*def_id, args, const_args, field)
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
