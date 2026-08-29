// SPDX-License-Identifier: GPL-3.0-or-later
//! Goal resolution, associated-item projection, and builtin implementations.

use super::*;

impl TraitSolver<'_> {
    pub(crate) fn is_enum(&self, ty: InternedTyId) -> bool {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return false;
        };
        if def_id.module_id == self.local_module_id {
            return self.local_enums.contains_key(&def_id.def_id);
        }
        self.program_is_enum
            .is_some_and(|program_is_enum| program_is_enum(*def_id))
    }

    /// Resolves a trait goal using assumptions, visible source impls, then
    /// compiler intrinsics, in that order.
    pub fn resolve(&mut self, goal: TraitGoal) -> TraitResolution {
        let goal = self.normalize_goal(goal);
        // Explicit assumptions describe the current generic environment and therefore outrank
        // global implementations. Visible user implementations outrank compiler-provided
        // behavior, so an intrinsic fallback never hides ambiguity among user candidates.
        if self
            .assumptions
            .iter()
            .any(|assumption| self.goals_equivalent(assumption, &goal))
        {
            return TraitResolution::Assumed(goal);
        }
        match self.select_user_impl_for_normalized_goal(&goal) {
            TraitSelection::User(user_impl) => return TraitResolution::User(user_impl),
            TraitSelection::Ambiguous => return TraitResolution::Ambiguous,
            TraitSelection::Unsatisfied => {}
        }
        if self.intrinsic_trait_impl_exists(&goal) {
            return TraitResolution::Intrinsic(IntrinsicImpl { goal });
        }
        TraitResolution::Unsatisfied
    }

    /// Selects only among visible source impls for `goal`.
    pub fn select_user_impl(&mut self, goal: TraitGoal) -> TraitSelection {
        let goal = self.normalize_goal(goal);
        self.select_user_impl_for_normalized_goal(&goal)
    }

    pub(crate) fn select_user_impl_for_normalized_goal(
        &mut self,
        goal: &TraitGoal,
    ) -> TraitSelection {
        // Trait implication is interpreted as a least fixed point. Re-entering
        // the same normalized goal through impl where-clauses is therefore not
        // a proof; only an assumption or a finite impl chain may establish it.
        if (0..self.active_goals.len()).any(|index| {
            let active = self.active_goals[index].clone();
            self.goals_equivalent(&active, goal)
        }) {
            return TraitSelection::Unsatisfied;
        }
        self.active_goals.push(goal.clone());
        let user_impls = self.matching_user_impls(goal);
        let selection = match user_impls.len() {
            0 => TraitSelection::Unsatisfied,
            1 => user_impls
                .into_iter()
                .next()
                .map(TraitSelection::User)
                .unwrap_or(TraitSelection::Unsatisfied),
            _ => TraitSelection::Ambiguous,
        };
        if let Some(index) = (0..self.active_goals.len()).rev().find(|index| {
            let active = self.active_goals[*index].clone();
            self.goals_equivalent(&active, goal)
        }) {
            self.active_goals.remove(index);
        }
        selection
    }

    /// Returns whether `goal` has a unique finite proof.
    ///
    /// Ambiguity is not proof, even if every remaining candidate would imply
    /// the same marker trait.
    pub fn proves(&mut self, goal: TraitGoal) -> bool {
        matches!(
            self.resolve(goal),
            TraitResolution::Intrinsic(_) | TraitResolution::User(_) | TraitResolution::Assumed(_)
        )
    }

    /// Resolves one associated type projection to its instantiated type.
    ///
    /// Returns `None` for missing items, ambiguous impls, assumption-only
    /// goals without a projection equality, and recursive projections.
    pub fn resolve_associated_type(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        self.resolve_associated_type_inner(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
            &mut Vec::new(),
        )
    }

    /// Resolves an associated const to either a source declaration instance or
    /// a compiler-provided value.
    pub fn resolve_associated_const(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<AssociatedConstResolution> {
        let goal = self.normalize_goal(TraitGoal {
            self_ty,
            trait_id,
            trait_args: trait_args.to_vec(),
            trait_const_args: trait_const_args.to_vec(),
        });
        let resolution = match self.select_user_impl_for_normalized_goal(&goal) {
            TraitSelection::User(user_impl) => TraitResolution::User(user_impl),
            TraitSelection::Ambiguous => TraitResolution::Ambiguous,
            TraitSelection::Unsatisfied => self.resolve(goal.clone()),
        };
        match resolution {
            TraitResolution::User(user_impl) => {
                let impl_signature = &self.trait_impls[user_impl.impl_index];
                let associated_value = impl_signature
                    .associated_values
                    .iter()
                    .find(|associated_value| &associated_value.name == name)?;
                Some(AssociatedConstResolution::User(Box::new(
                    UserAssociatedConst {
                        def_id: GlobalDefId {
                            module_id: impl_signature.module_id,
                            def_id: associated_value.def_id,
                        },
                        substitutions: user_impl.substitutions,
                        const_substitutions: user_impl.const_substitutions,
                        impl_module_id: impl_signature.module_id,
                    },
                )))
            }
            TraitResolution::Intrinsic(intrinsic) => self
                .resolve_intrinsic_associated_const(
                    intrinsic.goal.self_ty,
                    intrinsic.goal.trait_id,
                    &intrinsic.goal.trait_args,
                    name,
                )
                .map(AssociatedConstResolution::Const),
            TraitResolution::Assumed(_)
            | TraitResolution::Unsatisfied
            | TraitResolution::Ambiguous => None,
        }
    }

    pub(crate) fn resolve_associated_type_inner(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
        active: &mut Vec<AssociatedTypeProjectionKey>,
    ) -> Option<InternedTyId> {
        let goal = self.normalize_goal(TraitGoal {
            self_ty,
            trait_id,
            trait_args: trait_args.to_vec(),
            trait_const_args: trait_const_args.to_vec(),
        });
        let key = AssociatedTypeProjectionKey {
            goal: goal.clone(),
            name: *name,
        };
        // Recursive projections have no finite normal form. Keep them unresolved instead of
        // overflowing or manufacturing an equality from a cycle.
        if (0..active.len()).any(|index| {
            let active_key = active[index].clone();
            active_key.name == key.name && self.goals_equivalent(&active_key.goal, &key.goal)
        }) {
            return None;
        }
        active.push(key.clone());
        let assumed = self
            .associated_type_assumptions
            .iter()
            .find_map(|assumption| {
                if &assumption.name == name && self.goals_equivalent(&assumption.goal, &goal) {
                    Some(self.normalize(assumption.ty))
                } else {
                    None
                }
            });
        if let Some(assumed) = assumed {
            active.pop();
            return (!self.projection_matches_key(assumed, &key)).then_some(assumed);
        }
        let resolution = match self.select_user_impl_for_normalized_goal(&goal) {
            TraitSelection::User(user_impl) => TraitResolution::User(user_impl),
            TraitSelection::Ambiguous => TraitResolution::Ambiguous,
            TraitSelection::Unsatisfied => self.resolve(goal),
        };
        let resolved = match resolution {
            TraitResolution::User(user_impl) => {
                let impl_signature = &self.trait_impls[user_impl.impl_index];
                let Some(associated_type) = impl_signature
                    .associated_types
                    .iter()
                    .find(|associated_type| &associated_type.name == name)
                else {
                    // Keep the projection guard balanced even when a source
                    // impl matches the trait but omits the requested item.
                    active.pop();
                    return None;
                };
                Some(self.substitute_ty_with_consts(
                    associated_type.ty,
                    &user_impl.substitutions,
                    &user_impl.const_substitutions,
                ))
            }
            TraitResolution::Intrinsic(intrinsic) => self.resolve_intrinsic_associated_type(
                intrinsic.goal.self_ty,
                intrinsic.goal.trait_id,
                &intrinsic.goal.trait_args,
                name,
            ),
            TraitResolution::Assumed(_)
            | TraitResolution::Unsatisfied
            | TraitResolution::Ambiguous => None,
        };
        active.pop();
        resolved
    }

    pub(crate) fn projection_matches_key(
        &mut self,
        ty: InternedTyId,
        key: &AssociatedTypeProjectionKey,
    ) -> bool {
        match self.interner.get(self.normalize(ty)).cloned() {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                name == key.name
                    && trait_id == key.goal.trait_id
                    && trait_args.len() == key.goal.trait_args.len()
                    && trait_const_args.len() == key.goal.trait_const_args.len()
                    && self.types_equivalent(self_ty, key.goal.self_ty)
                    && trait_args
                        .iter()
                        .zip(&key.goal.trait_args)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
                    && trait_const_args
                        .iter()
                        .zip(&key.goal.trait_const_args)
                        .all(|(left, right)| self.const_generic_args_equivalent(left, right))
            }
            _ => false,
        }
    }

    pub(crate) fn const_generic_args_equivalent(
        &mut self,
        left: &ConstGenericArg,
        right: &ConstGenericArg,
    ) -> bool {
        self.types_equivalent(left.ty, right.ty)
            && self.const_generic_values_equivalent(left.ty, &left.value, &right.value)
    }

    pub(crate) fn array_lens_equivalent(&mut self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        if left == right {
            return true;
        }
        if let (
            ArrayLenTy::Builtin {
                builtin: left_builtin,
                ty: left_ty,
            },
            ArrayLenTy::Builtin {
                builtin: right_builtin,
                ty: right_ty,
            },
        ) = (left, right)
        {
            return left_builtin == right_builtin
                && self.structural_types_equivalent(*left_ty, *right_ty);
        }
        match (
            self.const_arg_from_array_len(left),
            self.const_arg_from_array_len(right),
        ) {
            (Some(left), Some(right)) => self.const_generic_args_equivalent(&left, &right),
            _ => false,
        }
    }

    pub(crate) fn const_generic_values_equivalent(
        &self,
        ty: InternedTyId,
        left: &ConstGenericValue,
        right: &ConstGenericValue,
    ) -> bool {
        match (
            self.resolve_const_generic_value(ty, left),
            self.resolve_const_generic_value(ty, right),
        ) {
            (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                left.bits() == right.bits()
            }
            (left_resolved, right_resolved) => left_resolved == right_resolved,
        }
    }

    pub(crate) fn resolve_const_generic_value(
        &self,
        ty: InternedTyId,
        value: &ConstGenericValue,
    ) -> ConstGenericValue {
        match value {
            ConstGenericValue::ConstExpr(id) => self
                .const_expr_value
                .and_then(|value| value(*id, ty))
                .unwrap_or_else(|| value.clone()),
            ConstGenericValue::GenericParam(_)
            | ConstGenericValue::Int(_)
            | ConstGenericValue::Bool(_)
            | ConstGenericValue::Char(_) => value.clone(),
        }
    }

    pub(crate) fn intrinsic_trait_impl_exists(&mut self, goal: &TraitGoal) -> bool {
        let TraitId::Builtin(trait_id) = goal.trait_id else {
            return false;
        };
        let self_ty = self.normalize(goal.self_ty);
        match trait_id {
            BuiltinTrait::Add
            | BuiltinTrait::Sub
            | BuiltinTrait::Mul
            | BuiltinTrait::Div
            | BuiltinTrait::Rem => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.types_equivalent(self_ty, *rhs_ty) && self.is_numeric(self_ty)
            }
            BuiltinTrait::BitAnd | BuiltinTrait::BitOr | BuiltinTrait::BitXor => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.types_equivalent(self_ty, *rhs_ty) && self.is_integer(self_ty)
            }
            BuiltinTrait::Shl | BuiltinTrait::Shr => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.intrinsic_shift_impl_exists(self_ty, *rhs_ty)
            }
            BuiltinTrait::Neg => goal.trait_args.is_empty() && self.is_numeric(self_ty),
            BuiltinTrait::BitNot => goal.trait_args.is_empty() && self.is_integer(self_ty),
            BuiltinTrait::Not => {
                goal.trait_args.is_empty() && self.types_equivalent(self_ty, self.bool())
            }
            BuiltinTrait::Eq => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.types_equivalent(self_ty, *rhs_ty)
                    && (self.is_numeric(self_ty)
                        || self.types_equivalent(self_ty, self.bool())
                        || self.is_char(self_ty)
                        || self.is_pointer(self_ty)
                        || self.is_enum(self_ty))
            }
            BuiltinTrait::Ord => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.types_equivalent(self_ty, *rhs_ty)
                    && (self.is_numeric(self_ty) || self.is_char(self_ty))
            }
            BuiltinTrait::Sized => goal.trait_args.is_empty() && self.layout_of(self_ty),
            BuiltinTrait::Unsized => {
                goal.trait_args.is_empty()
                    && (self.is_generic_param(self_ty) || self.is_unsized_pointee(self_ty))
            }
            BuiltinTrait::Deref => {
                goal.trait_args.is_empty()
                    && self.intrinsic_deref_target_ty(self_ty, false).is_some()
            }
            BuiltinTrait::DerefMut => {
                goal.trait_args.is_empty()
                    && self.intrinsic_deref_target_ty(self_ty, true).is_some()
            }
            BuiltinTrait::Index => {
                let [index_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, false)
                    .is_some()
            }
            BuiltinTrait::IndexMut => {
                let [index_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, true)
                    .is_some()
            }
            BuiltinTrait::Slice => {
                let [range_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.is_usize_range(*range_ty)
                    && self.intrinsic_slice_output_ty(self_ty, false).is_some()
            }
            BuiltinTrait::SliceMut => {
                let [range_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.is_usize_range(*range_ty)
                    && self.intrinsic_slice_output_ty(self_ty, true).is_some()
            }
            BuiltinTrait::Iterable => {
                goal.trait_args.is_empty()
                    && !matches!(
                        self.resolve(TraitGoal {
                            self_ty,
                            trait_id: TraitId::Builtin(BuiltinTrait::Iterator),
                            trait_args: Vec::new(),
                            trait_const_args: Vec::new(),
                        }),
                        TraitResolution::Unsatisfied | TraitResolution::Ambiguous
                    )
            }
            BuiltinTrait::Iterator => false,
            BuiltinTrait::Simd => {
                goal.trait_args.is_empty()
                    && matches!(self.kind(self_ty), Some(TyKind::Vector { .. }))
            }
            BuiltinTrait::SimdMask => {
                goal.trait_args.is_empty()
                    && matches!(
                        self.kind(self_ty),
                        Some(TyKind::Vector {
                            elem: PrimitiveTy::Bool,
                            lanes
                        }) if *lanes <= 64
                    )
            }
        }
    }

    pub(crate) fn resolve_intrinsic_associated_type(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let TraitId::Builtin(trait_id) = trait_id else {
            return None;
        };
        match (trait_id, builtin_associated_type_from_symbol(*name)?) {
            (
                BuiltinTrait::Add
                | BuiltinTrait::Sub
                | BuiltinTrait::Mul
                | BuiltinTrait::Div
                | BuiltinTrait::Rem
                | BuiltinTrait::BitAnd
                | BuiltinTrait::BitOr
                | BuiltinTrait::BitXor,
                BuiltinAssociatedType::Output,
            ) => {
                let [rhs_ty] = trait_args else {
                    return None;
                };
                (self.types_equivalent(self_ty, *rhs_ty)
                    && (self.is_numeric(self_ty) || self.is_integer(self_ty)))
                .then_some(self.normalize(self_ty))
            }
            (BuiltinTrait::Shl | BuiltinTrait::Shr, BuiltinAssociatedType::Output) => {
                let [rhs_ty] = trait_args else {
                    return None;
                };
                self.intrinsic_shift_impl_exists(self_ty, *rhs_ty)
                    .then_some(self.normalize(self_ty))
            }
            (BuiltinTrait::Neg, BuiltinAssociatedType::Output) => {
                trait_args.is_empty().then_some(())?;
                self.is_numeric(self_ty).then_some(self.normalize(self_ty))
            }
            (BuiltinTrait::BitNot, BuiltinAssociatedType::Output) => {
                trait_args.is_empty().then_some(())?;
                self.is_integer(self_ty).then_some(self.normalize(self_ty))
            }
            (BuiltinTrait::Deref, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_deref_target_ty(self_ty, false)
            }
            (BuiltinTrait::DerefMut, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_deref_target_ty(self_ty, true)
            }
            (BuiltinTrait::Index, BuiltinAssociatedType::Output) => {
                let [index_ty] = trait_args else {
                    return None;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, false)
            }
            (BuiltinTrait::IndexMut, BuiltinAssociatedType::Output) => {
                let [index_ty] = trait_args else {
                    return None;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, true)
            }
            (BuiltinTrait::Slice, BuiltinAssociatedType::Output) => {
                let [range_ty] = trait_args else {
                    return None;
                };
                self.is_usize_range(*range_ty).then_some(())?;
                self.intrinsic_slice_output_ty(self_ty, false)
            }
            (BuiltinTrait::SliceMut, BuiltinAssociatedType::Output) => {
                let [range_ty] = trait_args else {
                    return None;
                };
                self.is_usize_range(*range_ty).then_some(())?;
                self.intrinsic_slice_output_ty(self_ty, true)
            }
            (BuiltinTrait::Iterable, BuiltinAssociatedType::Item) => {
                trait_args.is_empty().then_some(())?;
                let item = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id: TraitId::Builtin(BuiltinTrait::Iterator),
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
                    name: known::ITEM,
                });
                Some(self.normalize(item))
            }
            (BuiltinTrait::Iterable, BuiltinAssociatedType::Iter) => {
                trait_args.is_empty().then_some(self.normalize(self_ty))
            }
            (BuiltinTrait::Simd, BuiltinAssociatedType::Lane) => {
                trait_args.is_empty().then_some(())?;
                match self.kind(self_ty) {
                    Some(TyKind::Vector { elem, .. }) => Some(self.interner.primitive(*elem)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(crate) fn resolve_intrinsic_associated_const(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &SymbolId,
    ) -> Option<ConstGenericArg> {
        let TraitId::Builtin(trait_id) = trait_id else {
            return None;
        };
        match (trait_id, builtin_associated_const_from_symbol(*name)?) {
            (BuiltinTrait::Simd, nia_ids::BuiltinAssociatedConst::Lanes) => {
                trait_args.is_empty().then_some(())?;
                let Some(TyKind::Vector { lanes, .. }) = self.kind(self_ty) else {
                    return None;
                };
                Some(ConstGenericArg {
                    ty: self.interner.primitive(PrimitiveTy::Usize),
                    value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(u128::from(*lanes))),
                })
            }
            _ => None,
        }
    }

    pub(crate) fn intrinsic_deref_target_ty(
        &mut self,
        self_ty: InternedTyId,
        require_mutable: bool,
    ) -> Option<InternedTyId> {
        match self.kind(self_ty) {
            Some(TyKind::Pointer {
                is_readonly: false,
                elem,
            }) if !self.is_unit(*elem) => Some(*elem),
            Some(TyKind::VolatilePointer {
                is_readonly: false,
                elem,
            }) if !self.is_unit(*elem) => Some(*elem),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            }) if !require_mutable && !self.is_unit(*elem) => Some(*elem),
            Some(TyKind::VolatilePointer {
                is_readonly: true,
                elem,
            }) if !require_mutable && !self.is_unit(*elem) => Some(*elem),
            _ => None,
        }
    }

    pub(crate) fn intrinsic_index_output_ty(
        &mut self,
        self_ty: InternedTyId,
        index_ty: InternedTyId,
        require_mutable: bool,
    ) -> Option<InternedTyId> {
        if !self.is_integer(index_ty) {
            return None;
        }
        match self.kind(self_ty) {
            Some(TyKind::Array { elem, .. }) => Some(*elem),
            Some(TyKind::Pointer {
                is_readonly: false,
                elem,
            })
            | Some(TyKind::VolatilePointer {
                is_readonly: false,
                elem,
            })
            | Some(TyKind::Slice {
                is_readonly: false,
                elem,
            }) => Some(*elem),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            })
            | Some(TyKind::VolatilePointer {
                is_readonly: true,
                elem,
            })
            | Some(TyKind::Slice {
                is_readonly: true,
                elem,
            }) if !require_mutable => Some(*elem),
            _ => None,
        }
    }

    pub(crate) fn intrinsic_slice_output_ty(
        &mut self,
        self_ty: InternedTyId,
        require_mutable: bool,
    ) -> Option<InternedTyId> {
        match self.kind(self_ty) {
            Some(TyKind::Array { elem, .. }) => Some(self.interner.intern(TyKind::Slice {
                is_readonly: !require_mutable,
                elem: *elem,
            })),
            Some(TyKind::Pointer {
                is_readonly: false,
                elem,
            })
            | Some(TyKind::VolatilePointer {
                is_readonly: false,
                elem,
            })
            | Some(TyKind::Slice {
                is_readonly: false,
                elem,
            }) => Some(self.interner.intern(TyKind::Slice {
                is_readonly: !require_mutable,
                elem: *elem,
            })),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            })
            | Some(TyKind::VolatilePointer {
                is_readonly: true,
                elem,
            })
            | Some(TyKind::Slice {
                is_readonly: true,
                elem,
            }) if !require_mutable => Some(self.interner.intern(TyKind::Slice {
                is_readonly: true,
                elem: *elem,
            })),
            _ => None,
        }
    }

    pub(crate) fn is_usize_range(&self, ty: InternedTyId) -> bool {
        let ty = self.normalize(ty);
        let Some(TyKind::Range { kind, bound }) = self.interner.get(ty) else {
            return false;
        };
        match (kind, bound) {
            (RangeTyKind::Full, None) => true,
            (_, Some(bound)) => self.structural_types_equivalent(*bound, self.usize()),
            _ => false,
        }
    }
}
