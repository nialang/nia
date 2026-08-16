// SPDX-License-Identifier: GPL-3.0-or-later
//! Trait selection, associated-item projection, and intrinsic trait semantics.
//!
//! User implementations are matched structurally and filtered by specificity;
//! compiler-provided implementations are considered only after assumptions and
//! visible user implementations. Projection and type equivalence share the same
//! normalization rules so selection cannot observe a different type identity.
//!
//! Resolution uses the following priority:
//!
//! | Candidate source | Meaning |
//! | --- | --- |
//! | Assumption | A bound supplied by the current generic environment |
//! | User impl | A visible impl whose target, arguments, and where-clauses match |
//! | Intrinsic | Compiler semantics for builtin traits such as `Sized` or `Add` |
//!
//! Matching a user impl produces both type and const substitutions. Those maps
//! are applied together to where-clauses and associated types; separating them
//! would leave types such as `[T; N]` partially instantiated. If several impls
//! match, selection keeps the maximal elements of the structural specificity
//! partial order. Multiple incomparable maxima are ambiguous.
//!
//! Where-clause proof is recursive but inductive: an impl cycle without an
//! assumption or finite base impl proves nothing. The solver tracks active
//! normalized goals to implement this least-fixed-point interpretation. Type
//! projection has a separate cycle guard because recursive associated types
//! likewise have no finite normal form.
#![warn(missing_docs)]

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
/// A fully applied trait proposition, before or after normalization.
pub struct TraitGoal {
    /// Type on the left-hand side of the trait relation.
    pub self_ty: InternedTyId,
    /// Source or builtin trait being requested.
    pub trait_id: TraitId,
    /// Type arguments supplied to the trait.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments supplied to the trait.
    pub trait_const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Equality for an associated-type projection supplied by a generic context.
pub struct AssociatedTypeProjectionEq {
    /// Trait proposition owning the projected item.
    pub goal: TraitGoal,
    /// Associated type name.
    pub name: SymbolId,
    /// Type known to equal the projection.
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AssociatedTypeProjectionKey {
    goal: TraitGoal,
    name: SymbolId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Complete result of resolving a trait goal.
pub enum TraitResolution {
    /// The compiler supplies the implementation.
    Intrinsic(IntrinsicImpl),
    /// A visible source impl was selected.
    User(UserImpl),
    /// The current generic environment states the goal directly.
    Assumed(TraitGoal),
    /// No finite proof exists.
    Unsatisfied,
    /// Multiple incomparable user impls remain.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of considering source impls without assumptions or intrinsics.
pub enum TraitSelection {
    /// Exactly one maximal source impl remains.
    User(UserImpl),
    /// No source impl matches and proves its where-clauses.
    Unsatisfied,
    /// Multiple incomparable maximal source impls remain.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Evidence that a normalized goal is implemented by compiler semantics.
pub struct IntrinsicImpl {
    /// Goal accepted by the intrinsic rules.
    pub goal: TraitGoal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A selected source impl and the instantiation inferred while matching it.
pub struct UserImpl {
    /// Normalized goal satisfied by this impl.
    pub goal: TraitGoal,
    /// Position in the `ProgramTraitImplSignature` slice supplied to the solver.
    pub impl_index: usize,
    /// Impl type parameters inferred from the goal.
    pub substitutions: SymbolMap<InternedTyId>,
    /// Impl const parameters inferred from the goal.
    pub const_substitutions: SymbolMap<ConstGenericArg>,
}

#[derive(Debug, Clone)]
/// Resolved source for an associated const projection.
pub enum AssociatedConstResolution {
    /// A source declaration that must be evaluated with the returned instance maps.
    User(Box<UserAssociatedConst>),
    /// A value supplied directly by compiler intrinsic semantics.
    Const(ConstGenericArg),
}

#[derive(Debug, Clone)]
/// Source associated const plus the impl instance needed to evaluate it.
pub struct UserAssociatedConst {
    /// Global definition of the associated const declaration.
    pub def_id: GlobalDefId,
    /// Inferred impl type arguments.
    pub substitutions: SymbolMap<InternedTyId>,
    /// Inferred impl const arguments.
    pub const_substitutions: SymbolMap<ConstGenericArg>,
    /// Module whose type context owns the impl declaration.
    pub impl_module_id: ModuleId,
}

/// Stateful resolver created from a [`TraitSolverContext`].
///
/// A solver owns per-session cycle state and appends synthesized types to the
/// local module's canonical type store. Create a fresh solver for each logical
/// set of assumptions.
pub struct TraitSolver<'a> {
    interner: TraitSolverTypeCx<'a>,
    active_goals: HashSet<TraitGoal>,
    normalization: &'a TypeNormalization,
    trait_impls: &'a [ProgramTraitImplSignature],
    trait_impl_index: Option<&'a ProgramTraitImplIndex>,
    assumptions: &'a [TraitGoal],
    associated_type_assumptions: &'a [AssociatedTypeProjectionEq],
    layouts: Option<&'a Layouts>,
    const_expr_value:
        Option<&'a dyn Fn(GlobalConstExprId, InternedTyId) -> Option<ConstGenericValue>>,
    local_module_id: ModuleId,
    local_enums: &'a HashMap<DefId, EnumSignature>,
    program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    impl_is_visible: &'a dyn Fn(ModuleId, TraitImplId) -> bool,
}

/// Immutable program facts from which solver sessions are created.
pub struct TraitSolverContext<'a> {
    /// Canonical program type store.
    pub type_store: &'a TypeStore,
    /// Alias and projection-independent type normalization product.
    pub normalization: &'a TypeNormalization,
    /// Source impls visible to the compilation pipeline.
    pub trait_impls: &'a [ProgramTraitImplSignature],
    /// Optional trait-to-impl acceleration index for `trait_impls`.
    pub trait_impl_index: Option<&'a ProgramTraitImplIndex>,
    /// Concrete layouts used by `Sized` and layout-backed const matching.
    pub layouts: Option<&'a Layouts>,
    /// Module that owns synthesized type identities for this session.
    pub local_module_id: ModuleId,
    /// Enum signatures owned by `local_module_id`.
    pub local_enums: &'a HashMap<DefId, EnumSignature>,
    /// Cross-module enum classifier.
    pub program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    /// Evaluates const-expression identities when const arguments are compared.
    pub const_expr_value:
        Option<&'a dyn Fn(GlobalConstExprId, InternedTyId) -> Option<ConstGenericValue>>,
    /// Restricts source impls according to module visibility.
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
    /// Creates a solver with trait goals assumed by the current generic scope.
    pub fn solver<'b>(&'b self, assumptions: &'b [TraitGoal]) -> TraitSolver<'b> {
        TraitSolver {
            interner: TraitSolverTypeCx {
                store: self.type_store,
                append: self.type_store.append_for_module(self.local_module_id),
            },
            active_goals: HashSet::new(),
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

    /// Creates a solver with trait and associated-projection assumptions.
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
            active_goals: HashSet::new(),
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
