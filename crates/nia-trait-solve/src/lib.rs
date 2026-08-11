// SPDX-License-Identifier: GPL-3.0-or-later
//! Trait selection, associated-item projection, and intrinsic trait semantics.
//!
//! User implementations are matched structurally and filtered by specificity;
//! compiler-provided implementations are considered only after assumptions and
//! visible user implementations. Projection and type equivalence share the same
//! normalization rules so selection cannot observe a different type identity.

mod equivalence;
mod layout;
mod overlap;
mod resolution;
mod selection;
mod substitution;

pub use overlap::IntrinsicOverlap;

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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
