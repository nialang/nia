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
        self.trait_object_vtables
            .get(&(self_ty, object_ty))
            .copied()
            .or_else(|| {
                self.trait_object_vtables
                    .iter()
                    .find(|((candidate_self, candidate_object), _)| {
                        self.same_type(*candidate_self, self_ty)
                            && self.same_type(*candidate_object, object_ty)
                    })
                    .map(|(_, global)| *global)
            })
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
        self.program
            .modules
            .values()
            .flat_map(|module| module.trait_object_vtables.iter())
            .find(|vtable| self.same_type(vtable.key.object_ty, object_ty))
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
        self.program
            .modules
            .values()
            .flat_map(|module| module.trait_object_vtables.iter())
            .find(|vtable| self.same_type(vtable.key.object_ty, source_ty))
            .and_then(|vtable| first_vtable_slot_for_trait(vtable, *target_trait))
            .unwrap_or(0)
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
