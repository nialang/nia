// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ids::{
    BuiltinAssociatedType, BuiltinTrait, DefId, GlobalConstExprId, GlobalDefId, InternedTyId,
    ModuleId, TraitId, TraitImplId,
};
use nia_item_signatures::{EnumSignature, ProgramTraitImplIndex, ProgramTraitImplSignature};
use nia_layout::Layouts;
use nia_symbol::{SymbolId, SymbolMap, known};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, PrimitiveTy, RangeTyKind, TyKind,
    TypeEquivalence, TypeStore, TypeStoreAppend,
};
use nia_type_normalize::TypeNormalization;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub self_ty: InternedTyId,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedTypeProjectionEq {
    pub goal: TraitGoal,
    pub name: SymbolId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AssociatedTypeProjectionKey {
    goal: TraitGoal,
    name: SymbolId,
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
pub enum TraitSelection {
    User(UserImpl),
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
    pub substitutions: SymbolMap<InternedTyId>,
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

#[derive(Debug, Clone)]
pub enum AssociatedConstResolution {
    User(Box<UserAssociatedConst>),
    Const(ConstGenericArg),
}

#[derive(Debug, Clone)]
pub struct UserAssociatedConst {
    pub def_id: GlobalDefId,
    pub substitutions: SymbolMap<InternedTyId>,
    pub const_substitutions: SymbolMap<ConstGenericArg>,
    pub impl_module_id: ModuleId,
}

pub struct TraitSolver<'a> {
    interner: TraitSolverTypeCx<'a>,
    pub normalization: &'a TypeNormalization,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub trait_impl_index: Option<&'a ProgramTraitImplIndex>,
    pub assumptions: &'a [TraitGoal],
    pub associated_type_assumptions: &'a [AssociatedTypeProjectionEq],
    pub layouts: Option<&'a Layouts>,
    pub const_expr_value:
        Option<&'a dyn Fn(GlobalConstExprId, InternedTyId) -> Option<ConstGenericValue>>,
    pub local_module_id: ModuleId,
    pub local_enums: &'a HashMap<DefId, EnumSignature>,
    pub program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    pub impl_is_visible: &'a dyn Fn(ModuleId, TraitImplId) -> bool,
}

pub struct TraitSolverContext<'a> {
    pub type_store: &'a TypeStore,
    pub normalization: &'a TypeNormalization,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub trait_impl_index: Option<&'a ProgramTraitImplIndex>,
    pub layouts: Option<&'a Layouts>,
    pub local_module_id: ModuleId,
    pub local_enums: &'a HashMap<DefId, EnumSignature>,
    pub program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    pub const_expr_value:
        Option<&'a dyn Fn(GlobalConstExprId, InternedTyId) -> Option<ConstGenericValue>>,
    pub impl_is_visible: Option<&'a dyn Fn(ModuleId, TraitImplId) -> bool>,
}

struct TraitSolverTypeCx<'a> {
    store: &'a TypeStore,
    append: TypeStoreAppend,
}

impl TraitSolverTypeCx<'_> {
    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }

    fn intern(&mut self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }

    fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.append.intern(TyKind::Primitive(primitive))
    }
}

fn builtin_associated_type_from_symbol(name: SymbolId) -> Option<BuiltinAssociatedType> {
    match name {
        known::OUTPUT => Some(BuiltinAssociatedType::Output),
        known::TARGET => Some(BuiltinAssociatedType::Target),
        known::ITEM => Some(BuiltinAssociatedType::Item),
        known::ITER => Some(BuiltinAssociatedType::Iter),
        known::LANE => Some(BuiltinAssociatedType::Lane),
        _ => None,
    }
}

fn builtin_associated_const_from_symbol(name: SymbolId) -> Option<nia_ids::BuiltinAssociatedConst> {
    match name {
        known::LANES => Some(nia_ids::BuiltinAssociatedConst::Lanes),
        _ => None,
    }
}

fn trait_impl_visible_by_default(_: ModuleId, _: TraitImplId) -> bool {
    true
}

impl<'a> TraitSolverContext<'a> {
    pub fn solver<'b>(&'b self, assumptions: &'b [TraitGoal]) -> TraitSolver<'b> {
        TraitSolver {
            interner: TraitSolverTypeCx {
                store: self.type_store,
                append: self.type_store.append_for_module(self.local_module_id),
            },
            normalization: self.normalization,
            trait_impls: self.trait_impls,
            trait_impl_index: self.trait_impl_index,
            assumptions,
            associated_type_assumptions: &[],
            layouts: self.layouts,
            const_expr_value: self.const_expr_value,
            local_module_id: self.local_module_id,
            local_enums: self.local_enums,
            program_is_enum: self.program_is_enum,
            impl_is_visible: self
                .impl_is_visible
                .unwrap_or(&trait_impl_visible_by_default),
        }
    }

    pub fn solver_with_associated_type_assumptions<'b>(
        &'b self,
        assumptions: &'b [TraitGoal],
        associated_type_assumptions: &'b [AssociatedTypeProjectionEq],
    ) -> TraitSolver<'b> {
        TraitSolver {
            interner: TraitSolverTypeCx {
                store: self.type_store,
                append: self.type_store.append_for_module(self.local_module_id),
            },
            normalization: self.normalization,
            trait_impls: self.trait_impls,
            trait_impl_index: self.trait_impl_index,
            assumptions,
            associated_type_assumptions,
            layouts: self.layouts,
            const_expr_value: self.const_expr_value,
            local_module_id: self.local_module_id,
            local_enums: self.local_enums,
            program_is_enum: self.program_is_enum,
            impl_is_visible: self
                .impl_is_visible
                .unwrap_or(&trait_impl_visible_by_default),
        }
    }
}

pub struct IntrinsicOverlap<'a, F>
where
    F: Fn(InternedTyId) -> bool,
{
    pub type_store: &'a TypeStore,
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
                match self.type_store.get(self.normalize(self_ty)) {
                    Some(TyKind::GenericParam(_)) => self.can_be_integer(rhs_ty),
                    Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                        match self.type_store.get(self.normalize(rhs_ty)) {
                            Some(TyKind::GenericParam(_)) => true,
                            Some(TyKind::Primitive(rhs)) => rhs.is_integer(),
                            _ => false,
                        }
                    }
                    Some(TyKind::Vector { elem, .. }) if elem.is_integer() => {
                        self.patterns_can_match(self_ty, rhs_ty)
                    }
                    _ => false,
                }
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
            BuiltinTrait::Unsized => self.can_be_compiler_classified_type(self_ty),
            BuiltinTrait::Deref => self.can_be_non_unit_pointer(self_ty, false),
            BuiltinTrait::DerefMut => self.can_be_non_unit_pointer(self_ty, true),
            BuiltinTrait::Index => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::IndexMut => {
                let Some(index_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true) && self.can_be_integer(index_ty)
            }
            BuiltinTrait::Slice => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, false)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::SliceMut => {
                let Some(range_ty) = trait_args.first().copied() else {
                    return false;
                };
                self.can_be_array_pointer_or_slice(self_ty, true)
                    && self.can_be_usize_range(range_ty)
            }
            BuiltinTrait::Iterable => false,
            BuiltinTrait::Iterator => false,
            BuiltinTrait::Simd => self.can_be_simd(self_ty),
            BuiltinTrait::SimdMask => self.can_be_simd_mask(self_ty),
        }
    }

    fn patterns_can_match(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if self.types_equivalent(left, right) {
            return true;
        }
        match (self.type_store.get(left), self.type_store.get(right)) {
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
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.patterns_can_match(*left_elem, *right_elem),
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
        match (self.type_store.get(left), self.type_store.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }

    fn can_be_numeric(&self, ty: InternedTyId) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Primitive(primitive))
            | Some(TyKind::Vector {
                elem: primitive, ..
            }) => primitive.is_integer() || primitive.is_float(),
            _ => false,
        }
    }

    fn can_be_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
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
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::I8
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
                        | PrimitiveTy::Bool,
                    ..
                })
        )
    }

    fn can_be_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn can_be_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    fn can_be_simd(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_)) | Some(TyKind::Vector { .. })
        )
    }

    fn can_be_simd_mask(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::Bool,
                    lanes: 0..=64
                })
        )
    }

    fn can_be_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
                | Some(
                    TyKind::Pointer { .. }
                        | TyKind::VolatilePointer { .. }
                        | TyKind::FunctionPointer { .. },
                )
        )
    }

    fn can_be_enum(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(self.normalize(ty)),
            Some(TyKind::GenericParam(_))
        ) || (self.is_enum)(ty)
    }

    fn can_have_known_layout(&self, ty: InternedTyId) -> bool {
        !matches!(
            self.kind(ty),
            Some(TyKind::Error | TyKind::Primitive(PrimitiveTy::Never))
        )
    }

    fn can_be_compiler_classified_type(&self, ty: InternedTyId) -> bool {
        !matches!(self.kind(ty), Some(TyKind::Error))
    }

    fn can_be_non_unit_pointer(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { is_readonly, elem })
            | Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                (!mutable || !*is_readonly)
                    && !self
                        .type_store
                        .get(self.normalize(*elem))
                        .is_some_and(TyKind::is_unit)
            }
            _ => false,
        }
    }

    fn can_be_array_pointer_or_slice(&self, ty: InternedTyId, mutable: bool) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) | Some(TyKind::Array { .. }) => true,
            Some(
                TyKind::Pointer { is_readonly, .. }
                | TyKind::VolatilePointer { is_readonly, .. }
                | TyKind::Slice { is_readonly, .. },
            ) => !mutable || !*is_readonly,
            _ => false,
        }
    }

    fn can_be_usize_range(&self, ty: InternedTyId) -> bool {
        match self.type_store.get(self.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Range { bound: None, .. }) => true,
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => matches!(
                self.type_store.get(self.normalize(*bound)),
                Some(TyKind::GenericParam(_)) | Some(TyKind::Primitive(PrimitiveTy::Usize))
            ),
            _ => false,
        }
    }

    fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }

    fn kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        let ty = self.normalize(ty);
        self.type_store.get(ty)
    }
}

impl TraitSolver<'_> {
    fn is_enum(&self, ty: InternedTyId) -> bool {
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

    pub fn resolve(&mut self, goal: TraitGoal) -> TraitResolution {
        let goal = self.normalize_goal(goal);
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

    pub fn select_user_impl(&mut self, goal: TraitGoal) -> TraitSelection {
        let goal = self.normalize_goal(goal);
        self.select_user_impl_for_normalized_goal(&goal)
    }

    fn select_user_impl_for_normalized_goal(&mut self, goal: &TraitGoal) -> TraitSelection {
        let user_impls = self.matching_user_impls(goal);
        if user_impls.len() > 1 {
            return TraitSelection::Ambiguous;
        }
        if let Some(user_impl) = user_impls.into_iter().next() {
            return TraitSelection::User(user_impl);
        }
        TraitSelection::Unsatisfied
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
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        self.resolve_associated_type_inner(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
            &mut HashSet::new(),
        )
    }

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

    fn resolve_associated_type_inner(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        name: &SymbolId,
        active: &mut HashSet<AssociatedTypeProjectionKey>,
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
        if !active.insert(key.clone()) {
            return None;
        }
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
            active.remove(&key);
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
                let associated_type = impl_signature
                    .associated_types
                    .iter()
                    .find(|associated_type| &associated_type.name == name)?;
                Some(self.substitute_ty(associated_type.ty, &user_impl.substitutions))
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

    fn projection_matches_key(
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

    fn const_generic_args_equivalent(
        &mut self,
        left: &ConstGenericArg,
        right: &ConstGenericArg,
    ) -> bool {
        self.types_equivalent(left.ty, right.ty)
            && self.const_generic_values_equivalent(left.ty, &left.value, &right.value)
    }

    fn const_generic_values_equivalent(
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

    fn resolve_const_generic_value(
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

    pub fn resolve_intrinsic_associated_type(
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

    pub fn resolve_intrinsic_associated_const(
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

    pub fn intrinsic_deref_target_ty(
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

    pub fn is_usize_range(&self, ty: InternedTyId) -> bool {
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

    pub fn types_equivalent(&mut self, left: InternedTyId, right: InternedTyId) -> bool {
        self.types_equivalent_resolving_projections(left, right, &mut HashSet::new())
    }

    fn types_equivalent_resolving_projections(
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

    fn resolve_projection_ty(&mut self, ty: InternedTyId) -> Option<InternedTyId> {
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

    fn structural_types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize(left);
        let right = self.normalize(right);
        if left == right {
            return true;
        }
        self.compute_same_type_for_equiv(left, right)
    }

    fn structural_types_equivalent_resolving_projections(
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
                left_len == right_len
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

    fn matching_user_impls(&mut self, goal: &TraitGoal) -> Vec<UserImpl> {
        let mut matches = Vec::new();
        if let Some(index) = self.trait_impl_index {
            for impl_index in index.indexes_for_trait(goal.trait_id).iter().copied() {
                if let Some(user_impl) = self.match_user_impl_at(goal, impl_index) {
                    matches.push(user_impl);
                }
            }
        } else {
            for impl_index in 0..self.trait_impls.len() {
                if let Some(user_impl) = self.match_user_impl_at(goal, impl_index) {
                    matches.push(user_impl);
                }
            }
        }
        self.filter_more_specific_user_impls(matches)
    }

    fn match_user_impl_at(&mut self, goal: &TraitGoal, impl_index: usize) -> Option<UserImpl> {
        let impl_signature = self.trait_impls.get(impl_index)?;
        if impl_signature.builtin.is_some() {
            return None;
        }
        if !(self.impl_is_visible)(impl_signature.module_id, impl_signature.impl_id) {
            return None;
        }
        if impl_signature.trait_id != goal.trait_id {
            return None;
        }
        let target_ty = impl_signature.target_ty;
        let trait_args = &impl_signature.trait_args;
        let trait_const_args = &impl_signature.trait_const_args;
        if trait_args.len() != goal.trait_args.len()
            || trait_const_args.len() != goal.trait_const_args.len()
        {
            return None;
        }
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        let target_matches = self.match_impl_pattern_with_consts(
            target_ty,
            goal.self_ty,
            &mut substitutions,
            &mut const_substitutions,
        );
        let trait_args_match = target_matches
            && trait_args
                .iter()
                .zip(&goal.trait_args)
                .all(|(actual, expected)| {
                    self.match_impl_pattern_with_consts(
                        *actual,
                        *expected,
                        &mut substitutions,
                        &mut const_substitutions,
                    )
                });
        let trait_const_args_match = trait_args_match
            && trait_const_args
                .iter()
                .zip(&goal.trait_const_args)
                .all(|(actual, expected)| {
                    self.match_const_impl_pattern(actual, expected, &mut const_substitutions)
                });
        let where_holds = trait_const_args_match
            && self.impl_where_predicates_hold(impl_signature, &substitutions);
        (target_matches && trait_args_match && trait_const_args_match && where_holds).then(|| {
            UserImpl {
                goal: goal.clone(),
                impl_index,
                substitutions,
                const_substitutions,
            }
        })
    }

    fn filter_more_specific_user_impls(&mut self, matches: Vec<UserImpl>) -> Vec<UserImpl> {
        matches
            .iter()
            .filter(|candidate| {
                !matches.iter().any(|other| {
                    other.impl_index != candidate.impl_index
                        && self.user_impl_more_specific(other.impl_index, candidate.impl_index)
                })
            })
            .cloned()
            .collect()
    }

    fn user_impl_more_specific(&mut self, specific_index: usize, general_index: usize) -> bool {
        let specific = &self.trait_impls[specific_index];
        let general = &self.trait_impls[general_index];
        if specific.trait_id != general.trait_id
            || specific.trait_args.len() != general.trait_args.len()
        {
            return false;
        }
        let specific_target = specific.target_ty;
        let general_target = general.target_ty;
        let target_subsumes = self.pattern_subsumes(general_target, specific_target);
        let target_strict = self.strictly_more_specific_pattern(specific_target, general_target);
        let mut any_strict = target_strict;
        let args_subsume = specific.trait_args.iter().zip(&general.trait_args).all(
            |(specific_arg, general_arg)| {
                any_strict |= self.strictly_more_specific_pattern(*specific_arg, *general_arg);
                self.pattern_subsumes(*general_arg, *specific_arg)
            },
        );
        target_subsumes && args_subsume && any_strict
    }

    fn strictly_more_specific_pattern(
        &mut self,
        specific: InternedTyId,
        general: InternedTyId,
    ) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    fn pattern_subsumes(&mut self, general: InternedTyId, specific: InternedTyId) -> bool {
        self.pattern_subsumes_inner(general, specific, &mut SymbolMap::default())
    }

    fn pattern_subsumes_inner(
        &mut self,
        general: InternedTyId,
        specific: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> bool {
        self.match_impl_pattern_with_consts(
            general,
            specific,
            substitutions,
            &mut SymbolMap::default(),
        )
    }

    fn impl_where_predicates_hold(
        &mut self,
        impl_signature: &ProgramTraitImplSignature,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        for predicate in &impl_signature.where_predicates {
            let self_ty = self.substitute_ty(predicate.ty, substitutions);
            for bound in &predicate.bounds {
                let trait_ty = self.substitute_ty(bound.trait_ty, substitutions);
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(trait_ty)
                else {
                    return false;
                };
                if !self.proves(TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                }) {
                    return false;
                }
                for binding in &bound.associated_type_bindings {
                    let binding_ty = self.substitute_ty(binding.ty, substitutions);
                    let Some(actual_ty) = self.resolve_associated_type(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &binding.name,
                    ) else {
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

    fn match_impl_pattern_with_consts(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        let pattern = self.normalize(pattern);
        let actual = self.normalize(actual);
        if let Some(resolved_pattern) = self.resolve_projection_ty(pattern)
            && resolved_pattern != pattern
        {
            return self.match_impl_pattern_with_consts(
                resolved_pattern,
                actual,
                substitutions,
                const_substitutions,
            );
        }
        if let Some(resolved_actual) = self.resolve_projection_ty(actual)
            && resolved_actual != actual
        {
            return self.match_impl_pattern_with_consts(
                pattern,
                resolved_actual,
                substitutions,
                const_substitutions,
            );
        }
        match self.interner.get(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_equivalent(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::SelfParam) => matches!(self.interner.get(actual), Some(TyKind::SelfParam)),
            Some(TyKind::BuiltinType(pattern_builtin)) => {
                matches!(self.interner.get(actual), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == *actual_builtin)
            }
            Some(TyKind::Opaque) => matches!(self.interner.get(actual), Some(TyKind::Opaque)),
            Some(TyKind::Tuple(pattern_elems)) => match self.interner.get(actual).cloned() {
                Some(TyKind::Tuple(actual_elems)) if pattern_elems.len() == actual_elems.len() => {
                    pattern_elems
                        .iter()
                        .zip(actual_elems)
                        .all(|(pattern_elem, actual_elem)| {
                            self.match_impl_pattern_with_consts(
                                *pattern_elem,
                                actual_elem,
                                substitutions,
                                const_substitutions,
                            )
                        })
                }
                _ => false,
            },
            Some(TyKind::Pointer { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::VolatilePointer { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::Slice { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::SlicePointee { elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::SlicePointee { elem: actual_elem })
                    if self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::Array { len, elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }) if self.match_array_len_pattern(&len, &actual_len, const_substitutions) => self
                    .match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    ),
                _ => false,
            },
            Some(TyKind::Range { kind, bound }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) if kind == actual_kind => match (bound, actual_bound) {
                    (Some(bound), Some(actual_bound)) => self.match_impl_pattern_with_consts(
                        bound,
                        actual_bound,
                        substitutions,
                        const_substitutions,
                    ),
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
                            self.match_impl_pattern_with_consts(
                                *param,
                                actual_param,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && self.match_impl_pattern_with_consts(
                            return_type,
                            actual_return,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Optional { elem: actual_elem }) => self
                    .match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    ),
                _ => false,
            },
            Some(TyKind::ErrorUnion { error, value }) => match self.interner.get(actual).cloned() {
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) => {
                    self.match_impl_pattern_with_consts(
                        error,
                        actual_error,
                        substitutions,
                        const_substitutions,
                    ) && self.match_impl_pattern_with_consts(
                        value,
                        actual_value,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }) if def_id == actual_def
                    && const_args.len() == actual_const_args.len()
                    && args.len() == actual_args.len() =>
                {
                    args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && const_args
                        .iter()
                        .zip(&actual_const_args)
                        .all(|(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
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
                            self.match_impl_pattern_with_consts(
                                *arg,
                                actual_arg,
                                substitutions,
                                const_substitutions,
                            )
                        })
                    }
                    _ => false,
                }
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) if is_readonly == actual_readonly
                    && trait_id == actual_trait
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len()
                    && associated_type_bindings.len() == actual_bindings.len() =>
                {
                    trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    ) && associated_type_bindings.iter().all(|binding| {
                        actual_bindings
                            .iter()
                            .find(|actual_binding| {
                                binding.name == actual_binding.name
                                    && binding.trait_id == actual_binding.trait_id
                                    && binding.trait_args.len() == actual_binding.trait_args.len()
                                    && binding.trait_const_args.len()
                                        == actual_binding.trait_const_args.len()
                            })
                            .is_some_and(|actual_binding| {
                                binding
                                    .trait_args
                                    .iter()
                                    .zip(&actual_binding.trait_args)
                                    .all(|(arg, actual_arg)| {
                                        self.match_impl_pattern_with_consts(
                                            *arg,
                                            *actual_arg,
                                            substitutions,
                                            const_substitutions,
                                        )
                                    })
                                    && binding
                                        .trait_const_args
                                        .iter()
                                        .zip(&actual_binding.trait_const_args)
                                        .all(|(arg, actual_arg)| {
                                            self.match_const_impl_pattern(
                                                arg,
                                                actual_arg,
                                                const_substitutions,
                                            )
                                        })
                                    && self.match_impl_pattern_with_consts(
                                        binding.ty,
                                        actual_binding.ty,
                                        substitutions,
                                        const_substitutions,
                                    )
                            })
                    })
                }
                _ => false,
            },
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) if trait_id == actual_trait
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len()
                    && associated_type_bindings.len() == actual_bindings.len() =>
                {
                    trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    ) && associated_type_bindings.iter().all(|binding| {
                        actual_bindings
                            .iter()
                            .find(|actual_binding| {
                                binding.name == actual_binding.name
                                    && binding.trait_id == actual_binding.trait_id
                                    && binding.trait_args.len() == actual_binding.trait_args.len()
                                    && binding.trait_const_args.len()
                                        == actual_binding.trait_const_args.len()
                            })
                            .is_some_and(|actual_binding| {
                                binding
                                    .trait_args
                                    .iter()
                                    .zip(&actual_binding.trait_args)
                                    .all(|(arg, actual_arg)| {
                                        self.match_impl_pattern_with_consts(
                                            *arg,
                                            *actual_arg,
                                            substitutions,
                                            const_substitutions,
                                        )
                                    })
                                    && binding
                                        .trait_const_args
                                        .iter()
                                        .zip(&actual_binding.trait_const_args)
                                        .all(|(arg, actual_arg)| {
                                            self.match_const_impl_pattern(
                                                arg,
                                                actual_arg,
                                                const_substitutions,
                                            )
                                        })
                                    && self.match_impl_pattern_with_consts(
                                        binding.ty,
                                        actual_binding.ty,
                                        substitutions,
                                        const_substitutions,
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
                trait_const_args,
                name,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }) if trait_id == actual_trait
                    && name == actual_name
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len() =>
                {
                    self.match_impl_pattern_with_consts(
                        self_ty,
                        actual_self,
                        substitutions,
                        const_substitutions,
                    ) && trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    )
                }
                _ => false,
            },
            Some(
                TyKind::Error | TyKind::ConstOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => self.types_equivalent(pattern, actual),
        }
    }

    fn match_const_impl_pattern(
        &mut self,
        pattern: &ConstGenericArg,
        actual: &ConstGenericArg,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if !self.types_equivalent(pattern.ty, actual.ty) {
            return false;
        }
        match &pattern.value {
            ConstGenericValue::GenericParam(name) => {
                if let Some(existing) = substitutions.get(name).cloned() {
                    self.const_generic_args_equivalent(&existing, actual)
                } else {
                    substitutions.insert(*name, actual.clone());
                    true
                }
            }
            _ => self.const_generic_args_equivalent(pattern, actual),
        }
    }

    fn match_array_len_pattern(
        &mut self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if pattern == actual {
            return true;
        }
        match (pattern, actual) {
            (ArrayLenTy::GenericParam(name), actual) => {
                let Some(actual_arg) = self.const_arg_from_array_len(actual) else {
                    return false;
                };
                if let Some(existing) = substitutions.get(name).cloned() {
                    self.const_generic_args_equivalent(&existing, &actual_arg)
                } else {
                    substitutions.insert(*name, actual_arg);
                    true
                }
            }
            _ => self.same_array_len_for_equiv(pattern, actual),
        }
    }

    fn const_arg_from_array_len(&self, len: &ArrayLenTy) -> Option<ConstGenericArg> {
        let ty = self.interner.primitive(PrimitiveTy::Usize);
        let value = match len {
            ArrayLenTy::ConstValue(value) => {
                ConstGenericValue::Int(nia_ty::IntConst::unsigned((*value).into()))
            }
            ArrayLenTy::GenericParam(name) => ConstGenericValue::GenericParam(*name),
            ArrayLenTy::ConstExpr(id) => ConstGenericValue::ConstExpr(*id),
            ArrayLenTy::Builtin {
                builtin,
                ty: layout_ty,
            } => {
                let layout_ty = self.normalize(*layout_ty);
                let layouts = self.layouts?;
                let layout = layouts.types.get(&layout_ty).cloned().or_else(|| {
                    layouts.types.iter().find_map(|(candidate, layout)| {
                        self.types_equivalent_in_layout_interner(
                            layout_ty,
                            *candidate,
                            layouts,
                            &mut HashSet::new(),
                        )
                        .then(|| layout.clone())
                    })
                })?;
                ConstGenericValue::Int(nia_ty::IntConst::unsigned(
                    layout.builtin_value(*builtin).into(),
                ))
            }
            ArrayLenTy::Infer => return None,
        };
        Some(ConstGenericArg { ty, value })
    }

    fn substitute_ty(
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

    fn trait_id_and_args(
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

    fn goals_equivalent(&mut self, left: &TraitGoal, right: &TraitGoal) -> bool {
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

    fn normalize_goal(&self, goal: TraitGoal) -> TraitGoal {
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

    fn normalize(&self, ty: InternedTyId) -> InternedTyId {
        self.normalization.normalize(ty)
    }

    fn kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(self.normalize(ty))
    }

    fn layout_of(&self, ty: InternedTyId) -> bool {
        let ty = self.normalize(ty);
        if self.intrinsic_sized_shape(ty) {
            return true;
        }
        let Some(layouts) = self.layouts else {
            return false;
        };
        if layouts.types.contains_key(&ty) {
            return true;
        }
        if self.layout_types_contain_equivalent(ty, layouts) {
            return true;
        }
        match self.kind(ty) {
            Some(TyKind::Nominal { def_id, args, .. }) => {
                layouts.nominal_type_layout(*def_id, args).is_some()
            }
            _ => false,
        }
    }

    fn intrinsic_sized_shape(&self, ty: InternedTyId) -> bool {
        match self.kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => false,
            Some(TyKind::Primitive(_) | TyKind::Vector { .. } | TyKind::FunctionPointer { .. }) => {
                true
            }
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::Slice { .. }
                | TyKind::Range { bound: None, .. },
            ) => true,
            Some(TyKind::SlicePointee { .. } | TyKind::GenericParam(_)) => false,
            Some(TyKind::Array {
                len: ArrayLenTy::ConstValue(_),
                elem,
            }) => self.intrinsic_sized_shape(*elem) || self.layout_of(*elem),
            Some(TyKind::Array { .. }) => false,
            Some(TyKind::Range {
                bound: Some(bound), ..
            })
            | Some(TyKind::Optional { elem: bound }) => {
                self.intrinsic_sized_shape(*bound) || self.layout_of(*bound)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                (self.intrinsic_sized_shape(*error) || self.layout_of(*error))
                    && (self.intrinsic_sized_shape(*value) || self.layout_of(*value))
            }
            _ => false,
        }
    }

    fn layout_types_contain_equivalent(&self, ty: InternedTyId, layouts: &Layouts) -> bool {
        layouts.types.keys().any(|layout_ty| {
            self.types_equivalent_in_layout_interner(ty, *layout_ty, layouts, &mut HashSet::new())
        })
    }

    fn types_equivalent_in_layout_interner(
        &self,
        left: InternedTyId,
        right: InternedTyId,
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        let left = self.normalize(left);
        if !seen.insert((left, right)) {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
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
                    && self.types_equivalent_in_layout_interner(
                        *left_elem,
                        *right_elem,
                        layouts,
                        seen,
                    )
            }
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.types_equivalent_in_layout_interner(*left_elem, *right_elem, layouts, seen),
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
                self.array_lens_equivalent_in_layout_interner(left_len, right_len, layouts, seen)
                    && self.types_equivalent_in_layout_interner(
                        *left_elem,
                        *right_elem,
                        layouts,
                        seen,
                    )
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
                            self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
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
                    && self.type_slices_equivalent_in_layout_interner(
                        left_params,
                        right_params,
                        layouts,
                        seen,
                    )
                    && self.types_equivalent_in_layout_interner(
                        *left_return,
                        *right_return,
                        layouts,
                        seen,
                    )
            }
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
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
                self.types_equivalent_in_layout_interner(*left_error, *right_error, layouts, seen)
                    && self.types_equivalent_in_layout_interner(
                        *left_value,
                        *right_value,
                        layouts,
                        seen,
                    )
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
                    && left_const_args == right_const_args
                    && self.type_slices_equivalent_in_layout_interner(
                        left_args, right_args, layouts, seen,
                    )
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
                    && self.type_slices_equivalent_in_layout_interner(
                        left_args, right_args, layouts, seen,
                    )
            }
            _ => false,
        }
    }

    fn type_slices_equivalent_in_layout_interner(
        &self,
        left: &[InternedTyId],
        right: &[InternedTyId],
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.types_equivalent_in_layout_interner(*left, *right, layouts, seen)
            })
    }

    fn array_lens_equivalent_in_layout_interner(
        &self,
        left: &ArrayLenTy,
        right: &ArrayLenTy,
        layouts: &Layouts,
        seen: &mut HashSet<(InternedTyId, InternedTyId)>,
    ) -> bool {
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
            (ArrayLenTy::ConstExpr(left), ArrayLenTy::ConstExpr(right)) => left == right,
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => {
                left_builtin == right_builtin
                    && self.types_equivalent_in_layout_interner(*left_ty, *right_ty, layouts, seen)
            }
            _ => false,
        }
    }

    fn is_generic_param(&self, ty: InternedTyId) -> bool {
        matches!(self.kind(ty), Some(TyKind::GenericParam(_)))
    }

    fn is_unsized_pointee(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(TyKind::SlicePointee { .. } | TyKind::TraitObjectPointee { .. })
        )
    }

    fn bool(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Bool)
    }

    fn usize(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Usize)
    }

    fn is_unit(&self, ty: InternedTyId) -> bool {
        self.kind(ty).is_some_and(TyKind::is_unit)
    }

    fn is_numeric(&self, ty: InternedTyId) -> bool {
        match self.kind(ty) {
            Some(TyKind::Primitive(primitive))
            | Some(TyKind::Vector {
                elem: primitive, ..
            }) => primitive.is_integer() || primitive.is_float(),
            _ => false,
        }
    }

    fn intrinsic_shift_impl_exists(&mut self, self_ty: InternedTyId, rhs_ty: InternedTyId) -> bool {
        match self.kind(self_ty) {
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => {
                matches!(self.kind(rhs_ty), Some(TyKind::Primitive(rhs)) if rhs.is_integer())
            }
            Some(TyKind::Vector { elem, .. }) if elem.is_integer() => {
                self.types_equivalent(self_ty, rhs_ty)
            }
            _ => false,
        }
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
            )) | Some(TyKind::Vector {
                elem: PrimitiveTy::I8
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
                    | PrimitiveTy::Bool,
                ..
            })
        )
    }

    fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(self.kind(ty), Some(TyKind::Primitive(PrimitiveTy::Char)))
    }

    fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.kind(ty),
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. }
            )
        )
    }
}

impl TypeEquivalence for TraitSolver<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.interner.get(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        left == right
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.structural_types_equivalent(left, right)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
