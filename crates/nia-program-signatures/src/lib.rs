// SPDX-License-Identifier: GPL-3.0-or-later
//! Program-wide views of module-local semantic signatures.
//!
//! This crate qualifies the declaration-only products from `nia-item-signatures`
//! with global definition identities and provides visibility-aware extension and
//! trait-implementation indexes. It does not perform body checking or reparse
//! source text.
mod analysis;

use std::collections::HashMap;

use nia_ids::GlobalDefId;
use nia_item_signatures::{
    ProgramConstSignature, ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature,
    ProgramStructSignature, ProgramTraitImplIndex, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramTypeAliasSignature, ProgramUnionSignature,
};
use nia_symbol::{SymbolId, SymbolMap};

pub use analysis::*;

/// Resolves a global definition id to its declaration signature.
pub trait ProgramSignatureLookup {
    /// Looks up a function signature.
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature>;
    /// Looks up a global/static signature.
    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature>;
    /// Looks up a compile-time constant signature.
    fn const_eval(&self, def_id: GlobalDefId) -> Option<ProgramConstSignature>;
    /// Looks up a struct signature.
    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature>;
    /// Looks up a union signature.
    /// Looks up a union signature.
    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature>;
    /// Looks up an enum signature.
    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature>;
    /// Looks up a trait signature.
    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature>;
    /// Looks up a type-alias signature.
    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature>;
    /// Returns traits containing a method with the requested name.
    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId>;
    /// Resolves a trait method definition to its owning trait and signature.
    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)>;

    /// Tests whether a function is present without retaining its signature.
    fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.function(def_id).is_some()
    }

    /// Tests whether a global is present.
    fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.global(def_id).is_some()
    }

    /// Tests whether a constant is present.
    fn has_const(&self, def_id: GlobalDefId) -> bool {
        self.const_eval(def_id).is_some()
    }

    /// Tests whether a struct is present.
    fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.struct_(def_id).is_some()
    }

    /// Tests whether a union is present.
    fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.union(def_id).is_some()
    }

    /// Tests whether an enum is present.
    fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enum_(def_id).is_some()
    }

    /// Tests whether a trait is present.
    fn has_trait(&self, def_id: GlobalDefId) -> bool {
        self.trait_(def_id).is_some()
    }

    /// Tests whether a type alias is present.
    fn has_type_alias(&self, def_id: GlobalDefId) -> bool {
        self.type_alias(def_id).is_some()
    }
}

/// Borrowed lookup and trait-implementation inputs for semantic consumers.
#[derive(Clone, Copy)]
pub struct ProgramSignatureContext<'a> {
    /// Global signature lookup implementation.
    pub lookup: &'a dyn ProgramSignatureLookup,
    /// Program-wide trait implementations in deterministic collection order.
    pub trait_impls: &'a [ProgramTraitImplSignature],
    /// Optional index over `trait_impls` for fast candidate lookup.
    pub trait_impl_index: Option<&'a ProgramTraitImplIndex>,
}

impl<'a> ProgramSignatureContext<'a> {
    /// Creates a context without a precomputed implementation index.
    pub fn new(
        lookup: &'a dyn ProgramSignatureLookup,
        trait_impls: &'a [ProgramTraitImplSignature],
    ) -> Self {
        Self {
            lookup,
            trait_impls,
            trait_impl_index: None,
        }
    }

    /// Creates a context with a precomputed implementation index.
    pub fn new_indexed(
        lookup: &'a dyn ProgramSignatureLookup,
        trait_impls: &'a [ProgramTraitImplSignature],
        trait_impl_index: &'a ProgramTraitImplIndex,
    ) -> Self {
        Self {
            lookup,
            trait_impls,
            trait_impl_index: Some(trait_impl_index),
        }
    }
}

/// A lookup that contains no program declarations.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyProgramSignatureLookup;

/// Shared empty lookup used by [`ProgramSignatureContext::empty`].
pub static EMPTY_PROGRAM_SIGNATURE_LOOKUP: EmptyProgramSignatureLookup =
    EmptyProgramSignatureLookup;

impl ProgramSignatureLookup for EmptyProgramSignatureLookup {
    fn function(&self, _def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        None
    }

    fn global(&self, _def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        None
    }

    fn const_eval(&self, _def_id: GlobalDefId) -> Option<ProgramConstSignature> {
        None
    }

    fn struct_(&self, _def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        None
    }

    fn union(&self, _def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        None
    }

    fn enum_(&self, _def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        None
    }

    fn trait_(&self, _def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        None
    }

    fn type_alias(&self, _def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        None
    }

    fn trait_ids_with_method_named(&self, _name: &SymbolId) -> Vec<GlobalDefId> {
        Vec::new()
    }

    fn trait_owning_method(
        &self,
        _method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        None
    }
}

impl ProgramSignatureContext<'static> {
    /// Returns an empty context with no declarations or implementations.
    pub fn empty() -> Self {
        Self {
            lookup: &EMPTY_PROGRAM_SIGNATURE_LOOKUP,
            trait_impls: &[],
            trait_impl_index: None,
        }
    }
}

/// Borrowed maps implementing [`ProgramSignatureLookup`].
#[derive(Debug, Clone, Copy)]
pub struct ProgramSignatureMaps<'a> {
    /// Function signatures by global definition id.
    pub functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    /// Global signatures by global definition id.
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    /// Constant signatures by global definition id.
    pub consts: &'a HashMap<GlobalDefId, ProgramConstSignature>,
    /// Struct signatures by global definition id.
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    /// Union signatures by global definition id.
    /// Union signatures by global definition id.
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    /// Enum signatures by global definition id.
    /// Enum signatures by global definition id.
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    /// Trait signatures by global definition id.
    /// Trait signatures by global definition id.
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    /// Type-alias signatures by global definition id.
    /// Type-alias signatures by global definition id.
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    /// Deterministic trait-method lookup index.
    pub trait_method_index: &'a ProgramTraitMethodIndex,
}

/// Borrowed non-function maps for consumers that do not need function lookup.
#[derive(Debug, Clone, Copy)]
pub struct ProgramNonFunctionSignatureMaps<'a> {
    /// Global signatures by global definition id.
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    /// Constant signatures by global definition id.
    pub consts: &'a HashMap<GlobalDefId, ProgramConstSignature>,
    /// Struct signatures by global definition id.
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    /// Union signatures by global definition id.
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    /// Enum signatures by global definition id.
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    /// Trait signatures by global definition id.
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    /// Type-alias signatures by global definition id.
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    /// Deterministic trait-method lookup index.
    pub trait_method_index: &'a ProgramTraitMethodIndex,
}

/// Index from trait method names and ids to their owning traits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgramTraitMethodIndex {
    trait_ids_by_method_name: SymbolMap<Vec<GlobalDefId>>,
    trait_id_by_method_id: HashMap<GlobalDefId, GlobalDefId>,
}

impl ProgramTraitMethodIndex {
    /// Builds an index from a complete trait map.
    pub fn from_traits(
        traits: &HashMap<GlobalDefId, ProgramTraitSignature>,
    ) -> ProgramTraitMethodIndex {
        Self::from_trait_signatures(
            traits
                .iter()
                .map(|(trait_id, signature)| (*trait_id, signature)),
        )
    }

    /// Builds an index from any iterable of trait signatures.
    pub fn from_trait_signatures<'a>(
        traits: impl IntoIterator<Item = (GlobalDefId, &'a ProgramTraitSignature)>,
    ) -> ProgramTraitMethodIndex {
        let mut trait_ids_by_method_name: SymbolMap<Vec<GlobalDefId>> = SymbolMap::default();
        let mut trait_id_by_method_id = HashMap::new();
        for (trait_id, signature) in traits {
            for method in &signature.signature.methods {
                trait_ids_by_method_name
                    .entry(method.name)
                    .or_default()
                    .push(trait_id);
                trait_id_by_method_id.insert(
                    GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    },
                    trait_id,
                );
            }
        }
        for trait_ids in trait_ids_by_method_name.values_mut() {
            trait_ids.sort();
            trait_ids.dedup();
        }
        ProgramTraitMethodIndex {
            trait_ids_by_method_name,
            trait_id_by_method_id,
        }
    }

    /// Returns sorted, deduplicated trait ids declaring `name`.
    pub fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_ids_by_method_name
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the trait owning a method definition id, if indexed.
    pub fn trait_owning_method_id(&self, method_id: GlobalDefId) -> Option<GlobalDefId> {
        self.trait_id_by_method_id.get(&method_id).copied()
    }
}

impl ProgramSignatureLookup for ProgramSignatureMaps<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        self.functions.get(&def_id).cloned()
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    fn const_eval(&self, def_id: GlobalDefId) -> Option<ProgramConstSignature> {
        self.consts.get(&def_id).cloned()
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_method_index.trait_ids_with_method_named(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.trait_method_index
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                self.traits
                    .get(&trait_id)
                    .cloned()
                    .map(|signature| (trait_id, signature))
            })
    }

    fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.functions.contains_key(&def_id)
    }

    fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.globals.contains_key(&def_id)
    }

    fn has_const(&self, def_id: GlobalDefId) -> bool {
        self.consts.contains_key(&def_id)
    }

    fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.structs.contains_key(&def_id)
    }

    fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.unions.contains_key(&def_id)
    }

    fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enums.contains_key(&def_id)
    }

    fn has_trait(&self, def_id: GlobalDefId) -> bool {
        self.traits.contains_key(&def_id)
    }

    fn has_type_alias(&self, def_id: GlobalDefId) -> bool {
        self.type_aliases.contains_key(&def_id)
    }
}

impl ProgramNonFunctionSignatureMaps<'_> {
    /// Looks up a global signature.
    pub fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.globals.get(&def_id).cloned()
    }

    /// Looks up a constant signature.
    pub fn const_eval(&self, def_id: GlobalDefId) -> Option<ProgramConstSignature> {
        self.consts.get(&def_id).cloned()
    }

    /// Looks up a struct signature.
    pub fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.structs.get(&def_id).cloned()
    }

    /// Looks up a union signature.
    pub fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.unions.get(&def_id).cloned()
    }

    /// Looks up an enum signature.
    pub fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.enums.get(&def_id).cloned()
    }

    /// Looks up a trait signature.
    pub fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.traits.get(&def_id).cloned()
    }

    /// Looks up a type-alias signature.
    pub fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.type_aliases.get(&def_id).cloned()
    }

    /// Returns traits declaring a method with the requested name.
    pub fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        self.trait_method_index.trait_ids_with_method_named(name)
    }

    /// Resolves a method id to its owning trait and signature.
    pub fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.trait_method_index
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                self.traits
                    .get(&trait_id)
                    .cloned()
                    .map(|signature| (trait_id, signature))
            })
    }
}

/// Function-backed implementation of [`ProgramSignatureLookup`].
#[derive(Clone, Copy)]
pub struct ProgramSignatureResolvers<'a> {
    /// Function signature resolver.
    pub function: &'a dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
    /// Global signature resolver.
    pub global: &'a dyn Fn(GlobalDefId) -> Option<ProgramGlobalSignature>,
    /// Constant signature resolver.
    pub const_eval: &'a dyn Fn(GlobalDefId) -> Option<ProgramConstSignature>,
    /// Struct signature resolver.
    pub struct_: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    /// Union signature resolver.
    pub union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    /// Enum signature resolver.
    pub enum_: &'a dyn Fn(GlobalDefId) -> Option<ProgramEnumSignature>,
    /// Trait signature resolver.
    pub trait_: &'a dyn Fn(GlobalDefId) -> Option<ProgramTraitSignature>,
    /// Type-alias signature resolver.
    pub type_alias: &'a dyn Fn(GlobalDefId) -> Option<ProgramTypeAliasSignature>,
    /// Trait-name index resolver.
    pub trait_ids_with_method_named: &'a dyn Fn(&SymbolId) -> Vec<GlobalDefId>,
    /// Trait-owner resolver for method ids.
    pub trait_owning_method:
        &'a dyn Fn(GlobalDefId) -> Option<(GlobalDefId, ProgramTraitSignature)>,
}

impl ProgramSignatureLookup for ProgramSignatureResolvers<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        (self.function)(def_id)
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        (self.global)(def_id)
    }

    fn const_eval(&self, def_id: GlobalDefId) -> Option<ProgramConstSignature> {
        (self.const_eval)(def_id)
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        (self.struct_)(def_id)
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        (self.union)(def_id)
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        (self.enum_)(def_id)
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        (self.trait_)(def_id)
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        (self.type_alias)(def_id)
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        (self.trait_ids_with_method_named)(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        (self.trait_owning_method)(method_id)
    }
}
