// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive generic substitution and normalized trait-goal construction.

use super::*;

impl TraitSolver<'_> {
    pub(crate) fn substitute_ty_with_consts(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> InternedTyId {
        let ty = self.normalize(ty);
        nia_ty::substitute_ty(
            self.interner.store,
            &self.interner.append,
            ty,
            &|name| substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        )
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
