// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive generic substitution and normalized trait-goal construction.

use super::*;

impl TraitSolver<'_> {
    pub(crate) fn substitute_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        let ty = self.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::SelfParam) => ty,
            Some(TyKind::Opaque) => ty,
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.substitute_ty(elem, substitutions))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            }) => {
                let captures = captures
                    .into_iter()
                    .map(|capture| self.substitute_ty(capture, substitutions))
                    .collect();
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_ty(param, substitutions))
                    .collect();
                let return_type = self.substitute_ty(return_type, substitutions);
                self.interner.intern(TyKind::ClosureState {
                    closure_id,
                    captures,
                    params,
                    return_type,
                })
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.substitute_ty(bound, substitutions));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_ty(param, substitutions))
                    .collect();
                let return_type = self.substitute_ty(return_type, substitutions);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_ty(param, substitutions))
                    .collect();
                let return_type = self.substitute_ty(return_type, substitutions);
                self.interner.intern(TyKind::Callable {
                    is_readonly,
                    params,
                    return_type,
                })
            }
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_ty(param, substitutions))
                    .collect();
                let return_type = self.substitute_ty(return_type, substitutions);
                self.interner.intern(TyKind::CallablePointee {
                    params,
                    return_type,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.substitute_ty(error, substitutions);
                let value = self.substitute_ty(value, substitutions);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_ty(arg, substitutions))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_ty(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_ty(arg, substitutions))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_ty(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.substitute_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. },
            )
            | None => ty,
        }
    }

    pub(crate) fn trait_id_and_args(
        &self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => Some((TraitId::Source(*def_id), args.clone(), const_args.clone())),
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) => Some((*trait_id, trait_args.clone(), trait_const_args.clone())),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
            }
            _ => None,
        }
    }

    pub(crate) fn goals_equivalent(&mut self, left: &TraitGoal, right: &TraitGoal) -> bool {
        left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && left.trait_const_args.len() == right.trait_const_args.len()
            && self.types_equivalent(left.self_ty, right.self_ty)
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| self.types_equivalent(*left, *right))
            && left
                .trait_const_args
                .iter()
                .zip(&right.trait_const_args)
                .all(|(left, right)| self.const_generic_args_equivalent(left, right))
    }

    pub(crate) fn normalize_goal(&self, goal: TraitGoal) -> TraitGoal {
        TraitGoal {
            self_ty: self.normalize(goal.self_ty),
            trait_id: goal.trait_id,
            trait_args: goal
                .trait_args
                .into_iter()
                .map(|arg| self.normalize(arg))
                .collect(),
            trait_const_args: goal
                .trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = self.normalize(arg.ty);
                    arg
                })
                .collect(),
        }
    }

    pub(crate) fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }

    pub(crate) fn kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(self.normalize(ty))
    }
}
