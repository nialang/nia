// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use nia_backend_ir::BackendTraitObjectVtable;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::{TraitId, TyKind};

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(crate) fn trait_object_vtable(
        &self,
        self_ty: InternedTyId,
        object_ty: InternedTyId,
    ) -> Option<nia_llvm::values::GlobalValue<'ctx>> {
        let key = (self_ty, object_ty);
        if let Some(global) = self.trait_object_vtables.get(&key).copied() {
            return Some(global);
        }
        if let Some(cached) = self.trait_object_vtable_lookups.borrow().get(&key) {
            return *cached;
        }
        let resolved = self
            .trait_object_vtables
            .iter()
            .find(|((candidate_self, candidate_object), _)| {
                self.same_type(*candidate_self, self_ty)
                    && self.same_type(*candidate_object, object_ty)
            })
            .map(|(_, global)| *global);
        self.trait_object_vtable_lookups
            .borrow_mut()
            .insert(key, resolved);
        resolved
    }

    pub(crate) fn trait_object_method_slot(
        &self,
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        slot: usize,
    ) -> usize {
        let Some(TyKind::TraitObject {
            trait_id: object_trait,
            ..
        }) = self.ty_kind(object_ty)
        else {
            return slot;
        };
        if *object_trait == trait_id {
            return slot;
        }
        self.trait_object_vtable_metadata(object_ty)
            .and_then(|vtable| vtable_slot(vtable, trait_id, method_id))
            .unwrap_or(slot)
    }

    pub(crate) fn trait_object_upcast_slot_offset(
        &self,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    ) -> usize {
        let Some(TyKind::TraitObject {
            trait_id: target_trait,
            ..
        }) = self.ty_kind(target_ty)
        else {
            return 0;
        };
        self.trait_object_vtable_metadata(source_ty)
            .and_then(|vtable| first_vtable_slot_for_trait(vtable, *target_trait))
            .unwrap_or(0)
    }

    fn trait_object_vtable_metadata(
        &self,
        object_ty: InternedTyId,
    ) -> Option<&BackendTraitObjectVtable> {
        let object_trait = match self.ty_kind(object_ty) {
            Some(TyKind::TraitObject { trait_id, .. }) => Some(*trait_id),
            _ => None,
        };
        self.program
            .trait_object_vtables_for_object_ty(object_ty)
            .find(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            .or_else(|| {
                self.program
                    .trait_object_vtables_for_trait(object_trait?)
                    .find(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            })
    }
}

fn vtable_slot(
    vtable: &BackendTraitObjectVtable,
    trait_id: TraitId,
    method_id: GlobalDefId,
) -> Option<usize> {
    vtable
        .entries
        .iter()
        .find(|entry| entry.trait_id == trait_id && entry.method_id == method_id)
        .map(|entry| entry.slot)
}

fn first_vtable_slot_for_trait(
    vtable: &BackendTraitObjectVtable,
    trait_id: TraitId,
) -> Option<usize> {
    vtable
        .entries
        .iter()
        .filter(|entry| entry.trait_id == trait_id)
        .map(|entry| entry.slot)
        .min()
}
