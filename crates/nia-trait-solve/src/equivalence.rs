// SPDX-License-Identifier: GPL-3.0-or-later
//! Normalized structural type equivalence with associated projection resolution.

use super::*;

impl TraitSolver<'_> {
    /// Compares normalized types, resolving associated projections when they
    /// have a finite, unambiguous definition.
    pub fn types_equivalent(&mut self, left: InternedTyId, right: InternedTyId) -> bool {
        self.types_equivalent_resolving_projections(left, right, &mut HashSet::new())
    }

    pub(crate) fn types_equivalent_resolving_projections(
        &mut self,
        left: InternedTyId,
        right: InternedTyId,
        active: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        // Re-entering a pair means projection resolution made no progress. These are not known
        // inductive recursive types, so an unresolved cycle is not evidence of equality.
        if !active.insert((left, right)) {
            return false;
        }
        if let Some(resolved_left) = self.resolve_projection_ty(left)
            && resolved_left != left
            && self.types_equivalent_resolving_projections(resolved_left, right, active)
        {
            active.remove(&(left, right));
            return true;
        }
        if let Some(resolved_right) = self.resolve_projection_ty(right)
            && resolved_right != right
            && self.types_equivalent_resolving_projections(left, resolved_right, active)
        {
            active.remove(&(left, right));
            return true;
        }
        let equivalent =
            self.structural_types_equivalent_resolving_projections(left, right, active);
        active.remove(&(left, right));
        equivalent
    }

    pub(crate) fn resolve_projection_ty(&mut self, ty: InternedTyId) -> Option<InternedTyId> {
        let TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        } = self.interner.get(self.normalize(ty)).cloned()?
        else {
            return None;
        };
        self.resolve_associated_type(self_ty, trait_id, &trait_args, &trait_const_args, &name)
    }

    pub(crate) fn structural_types_equivalent(
        &self,
        left: InternedTyId,
        right: InternedTyId,
    ) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        self.compute_same_type_for_equiv(left, right)
    }

    pub(crate) fn structural_types_equivalent_resolving_projections(
        &mut self,
        left: InternedTyId,
        right: InternedTyId,
        active: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        match (
            self.interner.get(left).cloned(),
            self.interner.get(right).cloned(),
        ) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::ConstOnly), Some(TyKind::ConstOnly)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (
                Some(TyKind::Vector {
                    elem: left,
                    lanes: left_lanes,
                }),
                Some(TyKind::Vector {
                    elem: right,
                    lanes: right_lanes,
                }),
            ) => left == right && left_lanes == right_lanes,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_readonly,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_readonly,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: left_readonly,
                    elem: left_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: right_readonly,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_readonly,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_readonly,
                    elem: right_elem,
                }),
            ) => {
                left_readonly == right_readonly
                    && self.types_equivalent_resolving_projections(left_elem, right_elem, active)
            }
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.types_equivalent_resolving_projections(left_elem, right_elem, active),
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
                self.array_lens_equivalent(&left_len, &right_len)
                    && self.types_equivalent_resolving_projections(left_elem, right_elem, active)
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
                        (Some(left), Some(right)) => {
                            self.types_equivalent_resolving_projections(left, right, active)
                        }
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
                    && left_params.len() == right_params.len()
                    && left_params.iter().zip(&right_params).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && self.types_equivalent_resolving_projections(
                        left_return,
                        right_return,
                        active,
                    )
            }
            (
                Some(TyKind::Callable {
                    is_readonly: left_readonly,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::Callable {
                    is_readonly: right_readonly,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_readonly == right_readonly
                    && left_params.len() == right_params.len()
                    && left_params.iter().zip(&right_params).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && self.types_equivalent_resolving_projections(
                        left_return,
                        right_return,
                        active,
                    )
            }
            (
                Some(TyKind::CallablePointee {
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::CallablePointee {
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_params.len() == right_params.len()
                    && left_params.iter().zip(&right_params).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && self.types_equivalent_resolving_projections(
                        left_return,
                        right_return,
                        active,
                    )
            }
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.types_equivalent_resolving_projections(left, right, active)
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
                self.types_equivalent_resolving_projections(left_error, right_error, active)
                    && self.types_equivalent_resolving_projections(left_value, right_value, active)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                    const_args: left_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                    const_args: right_const_args,
                }),
            ) => {
                left_def == right_def
                    && left_const_args.len() == right_const_args.len()
                    && left_args.len() == right_args.len()
                    && left_const_args
                        .iter()
                        .zip(&right_const_args)
                        .all(|(left, right)| self.const_generic_args_equivalent(left, right))
                    && left_args.iter().zip(&right_args).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => {
                left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(&right_args).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: left_readonly,
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_readonly,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_readonly == right_readonly
                    && left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_const_args.len() == right_const_args.len()
                    && left_bindings.len() == right_bindings.len()
                    && left_args.iter().zip(&right_args).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && left_const_args
                        .iter()
                        .zip(&right_const_args)
                        .all(|(left, right)| self.const_generic_args_equivalent(left, right))
                    && left_bindings.iter().all(|left_binding| {
                        right_bindings
                            .iter()
                            .find(|right_binding| {
                                left_binding.name == right_binding.name
                                    && left_binding.trait_id == right_binding.trait_id
                                    && left_binding.trait_args.len()
                                        == right_binding.trait_args.len()
                                    && left_binding.trait_const_args.len()
                                        == right_binding.trait_const_args.len()
                            })
                            .is_some_and(|right_binding| {
                                left_binding
                                    .trait_args
                                    .iter()
                                    .zip(&right_binding.trait_args)
                                    .all(|(left, right)| {
                                        self.types_equivalent_resolving_projections(
                                            *left, *right, active,
                                        )
                                    })
                                    && left_binding
                                        .trait_const_args
                                        .iter()
                                        .zip(&right_binding.trait_const_args)
                                        .all(|(left, right)| {
                                            self.const_generic_args_equivalent(left, right)
                                        })
                                    && self.types_equivalent_resolving_projections(
                                        left_binding.ty,
                                        right_binding.ty,
                                        active,
                                    )
                            })
                    })
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_const_args.len() == right_const_args.len()
                    && left_bindings.len() == right_bindings.len()
                    && left_args.iter().zip(&right_args).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && left_const_args
                        .iter()
                        .zip(&right_const_args)
                        .all(|(left, right)| self.const_generic_args_equivalent(left, right))
                    && left_bindings.iter().all(|left_binding| {
                        right_bindings
                            .iter()
                            .find(|right_binding| {
                                left_binding.name == right_binding.name
                                    && left_binding.trait_id == right_binding.trait_id
                                    && left_binding.trait_args.len()
                                        == right_binding.trait_args.len()
                                    && left_binding.trait_const_args.len()
                                        == right_binding.trait_const_args.len()
                            })
                            .is_some_and(|right_binding| {
                                left_binding
                                    .trait_args
                                    .iter()
                                    .zip(&right_binding.trait_args)
                                    .all(|(left, right)| {
                                        self.types_equivalent_resolving_projections(
                                            *left, *right, active,
                                        )
                                    })
                                    && left_binding
                                        .trait_const_args
                                        .iter()
                                        .zip(&right_binding.trait_const_args)
                                        .all(|(left, right)| {
                                            self.const_generic_args_equivalent(left, right)
                                        })
                                    && self.types_equivalent_resolving_projections(
                                        left_binding.ty,
                                        right_binding.ty,
                                        active,
                                    )
                            })
                    })
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && left_args.len() == right_args.len()
                    && left_const_args.len() == right_const_args.len()
                    && self.types_equivalent_resolving_projections(left_self, right_self, active)
                    && left_args.iter().zip(&right_args).all(|(left, right)| {
                        self.types_equivalent_resolving_projections(*left, *right, active)
                    })
                    && left_const_args
                        .iter()
                        .zip(&right_const_args)
                        .all(|(left, right)| self.const_generic_args_equivalent(left, right))
            }
            _ => false,
        }
    }
}
