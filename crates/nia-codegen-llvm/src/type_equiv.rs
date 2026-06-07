// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::InternedTyId;
use nia_ty::{ArrayLenTy, AssociatedTypeBindingTy, TyKind};

pub(crate) trait TypeEquivalence {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind>;
    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool;
    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool;

    fn same_type_args_for_equiv(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type_for_equiv(*left, *right))
    }

    fn compute_same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        match (self.ty_kind_for_equiv(left), self.ty_kind_for_equiv(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                self.same_array_len_for_equiv(left_len, right_len)
                    && self.same_type_for_equiv(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Range {
                    kind: left_kind,
                    bound: left_bound,
                }),
                Some(TyKind::Range {
                    kind: right_kind,
                    bound: right_bound,
                }),
            ) => {
                left_kind == right_kind
                    && match (left_bound, right_bound) {
                        (Some(left), Some(right)) => self.same_type_for_equiv(*left, *right),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => left_def == right_def && self.same_type_args_for_equiv(left_args, right_args),
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => left_trait == right_trait && self.same_type_args_for_equiv(left_args, right_args),
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.same_type_for_equiv(*left, *right)
            }
            (
                Some(TyKind::ErrorUnion {
                    error: left_error,
                    value: left_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: right_error,
                    value: right_value,
                }),
            ) => {
                self.same_type_for_equiv(*left_error, *right_error)
                    && self.same_type_for_equiv(*left_value, *right_value)
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: left_const,
                    trait_id: left_trait,
                    trait_args: left_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: left_trait,
                    trait_args: left_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: right_trait,
                    trait_args: right_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && self.same_type_for_equiv(*left_self, *right_self)
                    && self.same_type_args_for_equiv(left_args, right_args)
            }
            _ => false,
        }
    }

    fn same_associated_type_bindings_for_equiv(
        &self,
        left: &[AssociatedTypeBindingTy],
        right: &[AssociatedTypeBindingTy],
    ) -> bool {
        left.len() == right.len()
            && left.iter().all(|left_binding| {
                right
                    .iter()
                    .find(|right_binding| {
                        self.same_associated_type_binding_key_for_equiv(left_binding, right_binding)
                    })
                    .is_some_and(|right_binding| {
                        self.same_type_for_equiv(left_binding.ty, right_binding.ty)
                    })
            })
    }

    fn same_associated_type_binding_key_for_equiv(
        &self,
        left: &AssociatedTypeBindingTy,
        right: &AssociatedTypeBindingTy,
    ) -> bool {
        left.name == right.name
            && left.trait_id == right.trait_id
            && self.same_type_args_for_equiv(&left.trait_args, &right.trait_args)
    }
}
