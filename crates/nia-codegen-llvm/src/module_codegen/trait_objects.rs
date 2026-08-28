// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use nia_backend_ir::BackendTraitObjectVtable;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::{ConstGenericArg, TraitId, TyKind};

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
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        slot: usize,
    ) -> Result<usize, &'static str> {
        let Some(TyKind::TraitObject {
            trait_id: object_trait,
            ..
        }) = self.ty_kind(object_ty)
        else {
            return Err("dynamic trait call object type is not a trait object");
        };
        if *object_trait == trait_id {
            return Ok(slot);
        }
        self.trait_object_vtable_metadata(object_ty)
            .and_then(|vtable| {
                self.vtable_slot(vtable, trait_id, method_id, trait_args, trait_const_args)
            })
            .ok_or("dynamic trait call has no matching vtable method slot")
    }

    pub(crate) fn trait_object_upcast_slot_offset(
        &self,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
        span: nia_span::Span,
    ) -> Result<usize, nia_diagnostic::Diagnostic> {
        let Some(TyKind::TraitObject {
            trait_id: target_trait,
            trait_args,
            trait_const_args,
            ..
        }) = self.ty_kind(target_ty)
        else {
            return Err(self.error(span, "trait-object upcast target is not a trait object"));
        };
        self.trait_object_vtable_metadata(source_ty)
            .and_then(|vtable| {
                self.first_vtable_slot_for_trait(
                    vtable,
                    *target_trait,
                    trait_args,
                    trait_const_args,
                )
            })
            .ok_or_else(|| self.error(span, "trait-object upcast metadata is missing"))
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

    fn vtable_slot(
        &self,
        vtable: &BackendTraitObjectVtable,
        trait_id: TraitId,
        method_id: GlobalDefId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> Option<usize> {
        vtable
            .entries
            .iter()
            .find(|entry| {
                entry.trait_id == trait_id
                    && entry.method_id == method_id
                    && self.same_type_args(&entry.trait_args, trait_args)
                    && self.same_const_args(&entry.trait_const_args, trait_const_args)
            })
            .map(|entry| entry.slot)
    }

    fn first_vtable_slot_for_trait(
        &self,
        vtable: &BackendTraitObjectVtable,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> Option<usize> {
        vtable
            .entries
            .iter()
            .filter(|entry| {
                entry.trait_id == trait_id
                    && self.same_type_args(&entry.trait_args, trait_args)
                    && self.same_const_args(&entry.trait_const_args, trait_const_args)
            })
            .map(|entry| entry.slot)
            .min()
    }
}

/// Converts a vtable entry count to LLVM's array-length representation.
pub(crate) fn checked_vtable_array_len(entries: usize) -> Option<u32> {
    u32::try_from(entries).ok()
}

/// Converts an inclusive vtable slot into the LLVM array length needed by GEP.
pub(crate) fn checked_vtable_slot_array_len(slot: usize) -> Option<u32> {
    slot.checked_add(1)
        .and_then(|length| u32::try_from(length).ok())
}

/// Converts a host slot index to the width used by LLVM's integer index.
pub(crate) fn checked_vtable_index(slot: usize) -> Option<u64> {
    u64::try_from(slot).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_codegen::types::const_args_match_semantic;

    #[test]
    fn vtable_index_conversions_reject_llvm_width_overflow() {
        assert_eq!(checked_vtable_array_len(u32::MAX as usize), Some(u32::MAX));
        assert_eq!(checked_vtable_array_len(u32::MAX as usize + 1), None);
        assert_eq!(
            checked_vtable_slot_array_len(u32::MAX as usize - 1),
            Some(u32::MAX)
        );
        assert_eq!(checked_vtable_slot_array_len(u32::MAX as usize), None);
        assert_eq!(checked_vtable_index(42), Some(42));
    }

    #[test]
    fn const_argument_matching_ignores_integer_signedness() {
        let ty = InternedTyId::new(
            nia_ids::TypeStoreId::fresh(),
            nia_ids::TypeStoreIndex::from_store_index(0),
        );
        let signed = ConstGenericArg {
            ty,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::signed(3)),
        };
        let unsigned = ConstGenericArg {
            ty,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(3)),
        };
        assert!(const_args_match_semantic(
            &[signed],
            &[unsigned],
            |left, right| left == right,
        ));
    }
}
