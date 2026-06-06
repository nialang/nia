// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ids::{
    BuiltinAssociatedType, BuiltinTrait, DefId, GlobalDefId, InternedTyId, ModuleId, TraitId,
};
use nia_item_signatures::{EnumSignature, ProgramEnumSignature, ProgramTraitImplSignature};
use nia_layout::Layouts;
use nia_ty::{PrimitiveTy, RangeTyKind, TyInterner, TyKind, import_type_into};
use nia_type_normalize::TypeNormalization;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub self_ty: InternedTyId,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedTypeProjectionEq {
    pub goal: TraitGoal,
    pub name: String,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AssociatedTypeProjectionKey {
    goal: TraitGoal,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraitResolution {
    Intrinsic(IntrinsicImpl),
    User(UserImpl),
    Assumed(TraitGoal),
    Unsatisfied,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrinsicImpl {
    pub goal: TraitGoal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserImpl {
    pub goal: TraitGoal,
    pub impl_index: usize,
    pub substitutions: HashMap<String, InternedTyId>,
}

pub struct TraitSolver<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    pub interner: &'a mut TyInterner,
    pub normalization: &'a TypeNormalization,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub assumptions: &'a [TraitGoal],
    pub associated_type_assumptions: &'a [AssociatedTypeProjectionEq],
    pub layouts: Option<&'a Layouts>,
    pub is_enum: F,
}

pub struct TraitSolverContext<'a> {
    pub normalization: &'a TypeNormalization,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub layouts: Option<&'a Layouts>,
    pub local_module_id: ModuleId,
    pub local_enums: &'a HashMap<DefId, EnumSignature>,
    pub program_enums: Option<&'a HashMap<GlobalDefId, ProgramEnumSignature>>,
}

impl<'a> TraitSolverContext<'a> {
    pub fn solver<'b>(
        &'b self,
        interner: &'b mut TyInterner,
        assumptions: &'b [TraitGoal],
    ) -> TraitSolver<'b, impl Fn(InternedTyId) -> bool + 'b> {
        let interner_snapshot = interner.clone();
        TraitSolver {
            interner,
            normalization: self.normalization,
            trait_impls: self.trait_impls,
            assumptions,
            associated_type_assumptions: &[],
            layouts: self.layouts,
            is_enum: move |ty| self.is_enum_with_interner(&interner_snapshot, ty),
        }
    }

    pub fn solver_with_associated_type_assumptions<'b>(
        &'b self,
        interner: &'b mut TyInterner,
        assumptions: &'b [TraitGoal],
        associated_type_assumptions: &'b [AssociatedTypeProjectionEq],
    ) -> TraitSolver<'b, impl Fn(InternedTyId) -> bool + 'b> {
        let interner_snapshot = interner.clone();
        TraitSolver {
            interner,
            normalization: self.normalization,
            trait_impls: self.trait_impls,
            assumptions,
            associated_type_assumptions,
            layouts: self.layouts,
            is_enum: move |ty| self.is_enum_with_interner(&interner_snapshot, ty),
        }
    }

    fn is_enum_with_interner(&self, interner: &TyInterner, ty: InternedTyId) -> bool {
        let ty = self.normalization.normalize(ty);
        if ty.interner_id != interner.interner_id() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = interner.get(ty) else {
            return false;
        };
        if def_id.module_id == self.local_module_id {
            return self.local_enums.contains_key(&def_id.def_id);
        }
        self.program_enums
            .is_some_and(|program_enums| program_enums.contains_key(def_id))
    }
}

pub struct IntrinsicOverlap<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    pub interner: &'a TyInterner,
    pub normalization: &'a TypeNormalization,
    pub is_enum: F,
}

impl<'a, F> IntrinsicOverlap<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    pub fn overlaps_builtin_trait(
        &self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
    ) -> bool {
        let self_ty = self.normalize(self_ty);
        match trait_id {
            BuiltinTrait::Add
            | BuiltinTrait::Sub
            | BuiltinTrait::Mul
            | BuiltinTrait::Div
            | BuiltinTrait::Rem => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_numeric(self_ty) && self.patterns_can_match(self_ty, rhs_ty)
            }
            BuiltinTrait::BitAnd | BuiltinTrait::BitOr | BuiltinTrait::BitXor => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_integer(self_ty) && self.patterns_can_match(self_ty, rhs_ty)
            }
            BuiltinTrait::Shl | BuiltinTrait::Shr => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_integer(self_ty) && self.can_be_integer(rhs_ty)
            }
            BuiltinTrait::Neg => self.can_be_numeric(self_ty),
            BuiltinTrait::BitNot => self.can_be_integer(self_ty),
            BuiltinTrait::Not => self.can_be_bool(self_ty),
            BuiltinTrait::Eq => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.patterns_can_match(self_ty, rhs_ty)
                    && (self.can_be_numeric(self_ty)
                        || self.can_be_bool(self_ty)
                        || self.can_be_char(self_ty)
                        || self.can_be_pointer(self_ty)
                        || self.can_be_enum(self_ty))
            }
            BuiltinTrait::Ord => {
                let Some(rhs_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.patterns_can_match(self_ty, rhs_ty)
                    && (self.can_be_numeric(self_ty) || self.can_be_char(self_ty))
            }
            BuiltinTrait::Sized => self.can_have_known_layout(self_ty),
            BuiltinTrait::DerefRead => self.can_be_non_void_pointer(self_ty, false),
            BuiltinTrait::Deref => self.can_be_non_void_pointer(self_ty, true),
            BuiltinTrait::IndexRead => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::Index => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::SliceRead => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::Slice => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::GetPtrRead => self.can_be_slice(self_ty, false),
            BuiltinTrait::GetPtr => self.can_be_slice(self_ty, true),
        }
    }

    fn patterns_can_match(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if self.types_equivalent(left, right) {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::GenericParam(_)), _) | (_, Some(TyKind::GenericParam(_))) => true,
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
            ) => left_const == right_const && self.patterns_can_match(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    elem: left_elem, ..
                }),
                Some(TyKind::Array {
                    elem: right_elem, ..
                }),
            ) => self.patterns_can_match(*left_elem, *right_elem),
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
                        (Some(left_bound), Some(right_bound)) => {
                            self.patterns_can_match(*left_bound, *right_bound)
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }

    fn can_be_numeric(&self, ty: InternedTyId) -> bool {
        self.can_be_integer(ty)
            || matches!(
                self.interner.get(self.normalize(ty)),
                Some(TyKind::GenericParam(_))
                    | Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
            )
    }

    fn can_be_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(TyKind::Primitive(
                    PrimitiveTy::I8
                        | PrimitiveTy::I16
                        | PrimitiveTy::I32
                        | PrimitiveTy::I64
                        | PrimitiveTy::I128
                        | PrimitiveTy::Isize
                        | PrimitiveTy::U8
                        | PrimitiveTy::U16
                        | PrimitiveTy::U32
                        | PrimitiveTy::U64
                        | PrimitiveTy::U128
                        | PrimitiveTy::Usize
                ))
        )
    }

    fn can_be_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn can_be_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn can_be_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }

    fn can_be_enum(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
        ) || (self.is_enum)(ty)
    }

    fn can_have_known_layout(&self, ty: InternedTyId) -> bool {
        !matches!(
            self.interner.get(self.normalize(ty)),
            Some(TyKind::Error | TyKind::Primitive(PrimitiveTy::Never)) | None
        )
    }

    fn can_be_non_void_pointer(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { is_readonly, elem }) => {
                (!mutable || !*is_readonly)
                    && !matches!(
                        self.interner.get(self.normalize(*elem)),
                        Some(TyKind::Primitive(PrimitiveTy::Void))
                    )
            }
            _ => false,
        }
    }

    fn can_be_array_pointer_or_slice(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) | Some(TyKind::Array { .. }) => true,
            Some(TyKind::Pointer { is_readonly, .. } | TyKind::Slice { is_readonly, .. }) => {
                !mutable || !*is_readonly
            }
            _ => false,
        }
    }

    fn can_be_slice(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Slice { is_readonly, .. }) => !mutable || !*is_readonly,
            _ => false,
        }
    }

    fn can_be_usize_range(&self, ty: InternedTyId) -> bool {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Range { bound: None, .. }) => true,
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => matches!(
                self.interner.get(self.normalize(*bound)),
                Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Usize))
            ),
            _ => false,
        }
    }

    fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }
}

impl<'a, F> TraitSolver<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    pub fn resolve(&mut self, goal: TraitGoal) -> TraitResolution {
        let goal = self.normalize_goal(goal);
        if self
            .assumptions
            .iter()
            .any(|assumption| self.goals_equivalent(assumption, &goal))
        {
            return TraitResolution::Assumed(goal);
        }
        let user_impls = self.matching_user_impls(&goal);
        if user_impls.len() > 1 {
            return TraitResolution::Ambiguous;
        }
        if let Some(user_impl) = user_impls.into_iter().next() {
            return TraitResolution::User(user_impl);
        }
        if self.intrinsic_trait_impl_exists(&goal) {
            return TraitResolution::Intrinsic(IntrinsicImpl { goal });
        }
        TraitResolution::Unsatisfied
    }

    pub fn proves(&mut self, goal: TraitGoal) -> bool {
        matches!(
            self.resolve(goal),
            TraitResolution::Intrinsic(_) | TraitResolution::User(_) | TraitResolution::Assumed(_)
        )
    }

    pub fn resolve_associated_type(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        self.resolve_associated_type_inner(self_ty, trait_id, trait_args, name, &mut HashSet::new())
    }

    fn resolve_associated_type_inner(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &str,
        active: &mut HashSet<AssociatedTypeProjectionKey>,
    ) -> Option<InternedTyId> {
        let goal = self.normalize_goal(TraitGoal {
            self_ty,
            trait_id,
            trait_args: trait_args.to_vec(),
        });
        let key = AssociatedTypeProjectionKey {
            goal: goal.clone(),
            name: name.to_string(),
        };
        if !active.insert(key.clone()) {
            return None;
        }
        let assumed = self
            .associated_type_assumptions
            .iter()
            .find(|assumption| {
                assumption.name == name && self.goals_equivalent(&assumption.goal, &goal)
            })
            .map(|assumption| self.normalize(assumption.ty));
        if let Some(assumed) = assumed {
            active.remove(&key);
            return (!self.projection_matches_key(assumed, &key)).then_some(assumed);
        }
        let resolved = match self.resolve(goal) {
            TraitResolution::User(user_impl) => {
                let impl_signature = &self.trait_impls[user_impl.impl_index];
                let associated_type = impl_signature
                    .associated_types
                    .iter()
                    .find(|associated_type| associated_type.name == name)?;
                let ty =
                    import_type_into(self.interner, &impl_signature.interner, associated_type.ty);
                Some(self.substitute_ty(ty, &user_impl.substitutions))
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
        active.remove(&key);
        resolved
    }

    fn projection_matches_key(&self, ty: InternedTyId, key: &AssociatedTypeProjectionKey) -> bool {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                name == &key.name
                    && *trait_id == key.goal.trait_id
                    && trait_args.len() == key.goal.trait_args.len()
                    && self.types_equivalent(*self_ty, key.goal.self_ty)
                    && trait_args
                        .iter()
                        .zip(&key.goal.trait_args)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
            }
            _ => false,
        }
    }

    pub fn intrinsic_trait_impl_exists(&mut self, goal: &TraitGoal) -> bool {
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
                self.is_integer(self_ty) && self.is_integer(*rhs_ty)
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
                        || (self.is_enum)(self_ty))
            }
            BuiltinTrait::Ord => {
                let [rhs_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.types_equivalent(self_ty, *rhs_ty)
                    && (self.is_numeric(self_ty) || self.is_char(self_ty))
            }
            BuiltinTrait::Sized => goal.trait_args.is_empty() && self.layout_of(self_ty),
            BuiltinTrait::DerefRead => {
                goal.trait_args.is_empty()
                    && self.intrinsic_deref_target_ty(self_ty, false).is_some()
            }
            BuiltinTrait::Deref => {
                goal.trait_args.is_empty()
                    && self.intrinsic_deref_target_ty(self_ty, true).is_some()
            }
            BuiltinTrait::IndexRead => {
                let [index_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, false)
                    .is_some()
            }
            BuiltinTrait::Index => {
                let [index_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, true)
                    .is_some()
            }
            BuiltinTrait::SliceRead => {
                let [range_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.is_usize_range(*range_ty)
                    && self.intrinsic_slice_output_ty(self_ty, false).is_some()
            }
            BuiltinTrait::Slice => {
                let [range_ty] = goal.trait_args.as_slice() else {
                    return false;
                };
                self.is_usize_range(*range_ty)
                    && self.intrinsic_slice_output_ty(self_ty, true).is_some()
            }
            BuiltinTrait::GetPtrRead => {
                goal.trait_args.is_empty()
                    && self.intrinsic_get_ptr_target_ty(self_ty, false).is_some()
            }
            BuiltinTrait::GetPtr => {
                goal.trait_args.is_empty()
                    && self.intrinsic_get_ptr_target_ty(self_ty, true).is_some()
            }
        }
    }

    pub fn resolve_intrinsic_associated_type(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let TraitId::Builtin(trait_id) = trait_id else {
            return None;
        };
        if !trait_id.has_associated_type(name) {
            return None;
        }
        match (trait_id, BuiltinAssociatedType::from_name(name)?) {
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
                (self.is_integer(self_ty) && self.is_integer(*rhs_ty))
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
            (BuiltinTrait::DerefRead, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_deref_target_ty(self_ty, false)
            }
            (BuiltinTrait::Deref, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_deref_target_ty(self_ty, true)
            }
            (BuiltinTrait::IndexRead, BuiltinAssociatedType::Output) => {
                let [index_ty] = trait_args else {
                    return None;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, false)
            }
            (BuiltinTrait::Index, BuiltinAssociatedType::Output) => {
                let [index_ty] = trait_args else {
                    return None;
                };
                self.intrinsic_index_output_ty(self_ty, *index_ty, true)
            }
            (BuiltinTrait::SliceRead, BuiltinAssociatedType::Output) => {
                let [range_ty] = trait_args else {
                    return None;
                };
                self.is_usize_range(*range_ty).then_some(())?;
                self.intrinsic_slice_output_ty(self_ty, false)
            }
            (BuiltinTrait::Slice, BuiltinAssociatedType::Output) => {
                let [range_ty] = trait_args else {
                    return None;
                };
                self.is_usize_range(*range_ty).then_some(())?;
                self.intrinsic_slice_output_ty(self_ty, true)
            }
            (BuiltinTrait::GetPtrRead, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_get_ptr_target_ty(self_ty, false)
            }
            (BuiltinTrait::GetPtr, BuiltinAssociatedType::Target) => {
                trait_args.is_empty().then_some(())?;
                self.intrinsic_get_ptr_target_ty(self_ty, true)
            }
            _ => None,
        }
    }

    pub fn intrinsic_deref_target_ty(
        &mut self,
        self_ty: InternedTyId,
        require_mutable: bool,
    ) -> Option<InternedTyId> {
        match self.kind(self_ty) {
            Some(TyKind::Pointer {
                is_readonly: false,
                elem,
            }) if !self.is_void(*elem) => Some(*elem),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            }) if !require_mutable && !self.is_void(*elem) => Some(*elem),
            _ => None,
        }
    }

    pub fn intrinsic_index_output_ty(
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
            | Some(TyKind::Slice {
                is_readonly: false,
                elem,
            }) => Some(*elem),
            Some(TyKind::Pointer {
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

    pub fn intrinsic_slice_output_ty(
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

    pub fn intrinsic_get_ptr_target_ty(
        &mut self,
        self_ty: InternedTyId,
        require_mutable: bool,
    ) -> Option<InternedTyId> {
        match self.kind(self_ty) {
            Some(TyKind::Slice {
                is_readonly: false,
                elem,
            }) => Some(*elem),
            Some(TyKind::Slice {
                is_readonly: true,
                elem,
            }) if !require_mutable => Some(*elem),
            _ => None,
        }
    }

    pub fn is_usize_range(&self, ty: InternedTyId) -> bool {
        let ty = self.normalize(ty);
        let Some(TyKind::Range { kind, bound }) = self.interner.get(ty) else {
            return false;
        };
        match (kind, bound) {
            (RangeTyKind::Full, None) => true,
            (_, Some(bound)) => self.types_equivalent(*bound, self.usize()),
            _ => false,
        }
    }

    pub fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
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
            ) => left_const == right_const && self.types_equivalent(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => left_len == right_len && self.types_equivalent(*left_elem, *right_elem),
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
                        (Some(left_bound), Some(right_bound)) => {
                            self.types_equivalent(*left_bound, *right_bound)
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
                    && left_params
                        .iter()
                        .zip(right_params)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
                    && self.types_equivalent(*left_return, *right_return)
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
            ) => {
                left_def == right_def
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
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
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
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
                    && left_args.len() == right_args.len()
                    && self.types_equivalent(*left_self, *right_self)
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
            }
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }

    fn matching_user_impls(&mut self, goal: &TraitGoal) -> Vec<UserImpl> {
        let mut matches = Vec::new();
        for (impl_index, impl_signature) in self.trait_impls.iter().enumerate() {
            if impl_signature.trait_id != goal.trait_id {
                continue;
            }
            let target_ty = import_type_into(
                self.interner,
                &impl_signature.interner,
                impl_signature.target_ty,
            );
            let trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| import_type_into(self.interner, &impl_signature.interner, *arg))
                .collect::<Vec<_>>();
            if trait_args.len() != goal.trait_args.len() {
                continue;
            }
            let mut substitutions = HashMap::new();
            if self.match_impl_pattern(target_ty, goal.self_ty, &mut substitutions)
                && trait_args
                    .iter()
                    .zip(&goal.trait_args)
                    .all(|(actual, expected)| {
                        self.match_impl_pattern(*actual, *expected, &mut substitutions)
                    })
                && self.impl_where_predicates_hold(impl_signature, &substitutions)
            {
                matches.push(UserImpl {
                    goal: goal.clone(),
                    impl_index,
                    substitutions,
                });
            }
        }
        matches
    }

    fn impl_where_predicates_hold(
        &mut self,
        impl_signature: &ProgramTraitImplSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        for predicate in &impl_signature.where_predicates {
            let self_ty = import_type_into(self.interner, &impl_signature.interner, predicate.ty);
            let self_ty = self.substitute_ty(self_ty, substitutions);
            for bound in &predicate.bounds {
                let trait_ty =
                    import_type_into(self.interner, &impl_signature.interner, bound.trait_ty);
                let trait_ty = self.substitute_ty(trait_ty, substitutions);
                let Some((trait_id, trait_args)) = self.trait_id_and_args(trait_ty) else {
                    return false;
                };
                if !self.proves(TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                }) {
                    return false;
                }
                for binding in &bound.associated_type_bindings {
                    let binding_ty =
                        import_type_into(self.interner, &impl_signature.interner, binding.ty);
                    let binding_ty = self.substitute_ty(binding_ty, substitutions);
                    let Some(actual_ty) =
                        self.resolve_associated_type(self_ty, trait_id, &trait_args, &binding.name)
                    else {
                        return false;
                    };
                    if !self.types_equivalent(actual_ty, binding_ty) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn match_impl_pattern(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let pattern = self.normalize(pattern);
        let actual = self.normalize(actual);
        match self.interner.get(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_equivalent(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::Pointer { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern(elem, actual_elem, substitutions)
            ),
            Some(TyKind::Slice { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern(elem, actual_elem, substitutions)
            ),
            Some(TyKind::Array { len, elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }) if len == actual_len => {
                    self.match_impl_pattern(elem, actual_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range { kind, bound }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) if kind == actual_kind => match (bound, actual_bound) {
                    (Some(bound), Some(actual_bound)) => {
                        self.match_impl_pattern(bound, actual_bound, substitutions)
                    }
                    (None, None) => true,
                    _ => false,
                },
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                    params
                        .iter()
                        .zip(actual_params)
                        .all(|(param, actual_param)| {
                            self.match_impl_pattern(*param, actual_param, substitutions)
                        })
                        && self.match_impl_pattern(return_type, actual_return, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Optional { elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Optional { elem: actual_elem }) => {
                    self.match_impl_pattern(elem, actual_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::ErrorUnion { error, value }) => match self.interner.get(actual).cloned() {
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) => {
                    self.match_impl_pattern(error, actual_error, substitutions)
                        && self.match_impl_pattern(value, actual_value, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal { def_id, args }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                }) if def_id == actual_def && args.len() == actual_args.len() => {
                    args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern(*arg, actual_arg, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                match self.interner.get(actual).cloned() {
                    Some(TyKind::BuiltinTrait {
                        trait_id: actual_trait,
                        args: actual_args,
                    }) if trait_id == actual_trait && args.len() == actual_args.len() => {
                        args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                            self.match_impl_pattern(*arg, actual_arg, substitutions)
                        })
                    }
                    _ => false,
                }
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    associated_type_bindings: actual_bindings,
                }) if is_readonly == actual_readonly
                    && trait_id == actual_trait
                    && trait_args.len() == actual_args.len()
                    && associated_type_bindings.len() == actual_bindings.len() =>
                {
                    trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern(*arg, actual_arg, substitutions)
                    }) && associated_type_bindings.iter().all(|binding| {
                        actual_bindings
                            .iter()
                            .find(|actual_binding| {
                                binding.name == actual_binding.name
                                    && binding.trait_id == actual_binding.trait_id
                                    && binding.trait_args.len() == actual_binding.trait_args.len()
                            })
                            .is_some_and(|actual_binding| {
                                binding
                                    .trait_args
                                    .iter()
                                    .zip(&actual_binding.trait_args)
                                    .all(|(arg, actual_arg)| {
                                        self.match_impl_pattern(*arg, *actual_arg, substitutions)
                                    })
                                    && self.match_impl_pattern(
                                        binding.ty,
                                        actual_binding.ty,
                                        substitutions,
                                    )
                            })
                    })
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    name: actual_name,
                }) if trait_id == actual_trait
                    && name == actual_name
                    && trait_args.len() == actual_args.len() =>
                {
                    self.match_impl_pattern(self_ty, actual_self, substitutions)
                        && trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                            self.match_impl_pattern(*arg, actual_arg, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => {
                self.types_equivalent(pattern, actual)
            }
        }
    }

    fn substitute_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        let ty = self.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
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
            Some(TyKind::Optional { elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.substitute_ty(error, substitutions);
                let value = self.substitute_ty(value, substitutions);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
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
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
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
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.substitute_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                })
            }
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => ty,
        }
    }

    fn trait_id_and_args(&self, ty: InternedTyId) -> Option<(TraitId, Vec<InternedTyId>)> {
        match self.interner.get(self.normalize(ty)) {
            Some(TyKind::Nominal { def_id, args }) => {
                Some((TraitId::Source(*def_id), args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone()))
            }
            _ => None,
        }
    }

    fn goals_equivalent(&self, left: &TraitGoal, right: &TraitGoal) -> bool {
        left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && self.types_equivalent(left.self_ty, right.self_ty)
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| self.types_equivalent(*left, *right))
    }

    fn normalize_goal(&self, goal: TraitGoal) -> TraitGoal {
        TraitGoal {
            self_ty: self.normalize(goal.self_ty),
            trait_id: goal.trait_id,
            trait_args: goal
                .trait_args
                .into_iter()
                .map(|arg| self.normalize(arg))
                .collect(),
        }
    }

    fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }

    fn kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(self.normalize(ty))
    }

    fn layout_of(&self, ty: InternedTyId) -> bool {
        self.layouts
            .is_some_and(|layouts| layouts.types.contains_key(&self.normalize(ty)))
    }

    fn bool(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Bool)
    }

    fn usize(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Usize)
    }

    fn is_void(&self, ty: InternedTyId) -> bool {
        self.types_equivalent(ty, self.interner.primitive(PrimitiveTy::Void))
    }

    fn is_numeric(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.kind(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
            )
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(self.kind(ty), Some(TyKind::Primitive(PrimitiveTy::Char)))
    }

    fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }
}
