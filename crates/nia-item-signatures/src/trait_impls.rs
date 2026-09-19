//! Trait implementation identity and candidate indexing.

use super::*;
use nia_ty::{ArrayLenTy, ConstGenericValue};

/// Compact candidate index used by program-wide trait selection.
///
/// Values are indexes into the owning `ProgramTraitImplSignature` slice rather
/// than copied signatures, so the slice remains the only ordered source of
/// truth for implementation facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramTraitImplIndex {
    by_trait: HashMap<TraitId, Vec<usize>>,
    by_trait_target: HashMap<(TraitId, InternedTyId), Vec<usize>>,
    fallback_by_trait: HashMap<TraitId, Vec<usize>>,
}

impl ProgramTraitImplIndex {
    /// Builds a trait-to-candidate index over an ordered signature slice.
    pub fn new(trait_impls: &[ProgramTraitImplSignature]) -> Self {
        let mut by_trait = HashMap::<TraitId, Vec<usize>>::new();
        for (index, impl_signature) in trait_impls.iter().enumerate() {
            by_trait
                .entry(impl_signature.trait_id)
                .or_default()
                .push(index);
        }
        Self {
            by_trait,
            by_trait_target: HashMap::new(),
            fallback_by_trait: HashMap::new(),
        }
    }

    /// Builds an index that also filters concrete implementation targets.
    ///
    /// Implementations whose target contains a type or const parameter remain
    /// in the per-trait fallback bucket because they can match many goals.
    pub fn new_with_type_store(
        trait_impls: &[ProgramTraitImplSignature],
        type_store: &TypeStore,
    ) -> Self {
        let mut index = Self::new(trait_impls);
        for (impl_index, impl_signature) in trait_impls.iter().enumerate() {
            if type_contains_pattern(type_store, impl_signature.target_ty) {
                index
                    .fallback_by_trait
                    .entry(impl_signature.trait_id)
                    .or_default()
                    .push(impl_index);
            } else {
                index
                    .by_trait_target
                    .entry((impl_signature.trait_id, impl_signature.target_ty))
                    .or_default()
                    .push(impl_index);
            }
        }
        index
    }

    /// Returns candidate indexes for one trait, preserving source order.
    pub fn indexes_for_trait(&self, trait_id: TraitId) -> &[usize] {
        self.by_trait
            .get(&trait_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns concrete-target candidates plus generic fallback candidates in
    /// the original implementation order.
    pub fn indexes_for_trait_and_target(
        &self,
        trait_id: TraitId,
        target_ty: InternedTyId,
    ) -> Vec<usize> {
        if self.by_trait_target.is_empty() && self.fallback_by_trait.is_empty() {
            return self.indexes_for_trait(trait_id).to_vec();
        }
        let mut indexes = self
            .fallback_by_trait
            .get(&trait_id)
            .cloned()
            .unwrap_or_default();
        if let Some(concrete) = self.by_trait_target.get(&(trait_id, target_ty)) {
            indexes.extend(concrete);
        }
        indexes.sort_unstable();
        indexes.dedup();
        indexes
    }

    /// Reports whether no trait implementation candidates are indexed.
    pub fn is_empty(&self) -> bool {
        self.by_trait.is_empty()
    }
}

fn type_contains_pattern(type_store: &TypeStore, ty: InternedTyId) -> bool {
    fn visit(type_store: &TypeStore, ty: InternedTyId, seen: &mut Vec<InternedTyId>) -> bool {
        if seen.contains(&ty) {
            return false;
        }
        seen.push(ty);
        let Some(kind) = type_store.get(ty) else {
            return true;
        };
        let result = match kind {
            TyKind::GenericParam(_) | TyKind::SelfParam => true,
            TyKind::Nominal {
                args, const_args, ..
            } => {
                args.iter().any(|arg| visit(type_store, *arg, seen))
                    || const_args.iter().any(|arg| {
                        matches!(
                            arg.value,
                            ConstGenericValue::GenericParam(_) | ConstGenericValue::ConstExpr(_)
                        ) || visit(type_store, arg.ty, seen)
                    })
            }
            TyKind::Array { len, elem } => {
                matches!(len, ArrayLenTy::GenericParam(_) | ArrayLenTy::ConstExpr(_))
                    || matches!(len, ArrayLenTy::Builtin { ty, .. } if visit(type_store, *ty, seen))
                    || visit(type_store, *elem, seen)
            }
            _ => {
                let mut nested = false;
                kind.visit_referenced_types(|referenced| {
                    nested |= visit(type_store, referenced, seen);
                });
                nested
            }
        };
        seen.pop();
        result
    }

    visit(type_store, ty, &mut Vec::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TraitImplIdentity {
    target: String,
    trait_ref: Option<String>,
    generics: Vec<String>,
    where_clause: Vec<(String, Vec<String>)>,
    duplicate_ordinal: Option<u32>,
}

impl TraitImplIdentity {
    pub(super) fn from_extend(extend: &ExtendItem) -> Self {
        Self {
            target: type_ref_identity(&extend.target),
            trait_ref: extend.trait_ref.as_ref().map(type_ref_identity),
            generics: generic_param_identities(&extend.generics),
            where_clause: where_clause_identity(&extend.where_clause),
            duplicate_ordinal: None,
        }
    }

    pub(super) fn duplicate(mut self, ordinal: u32) -> Self {
        self.duplicate_ordinal = Some(ordinal);
        self
    }

    pub(super) fn display(&self) -> String {
        let trait_ref = self.trait_ref.as_deref().unwrap_or("inherent");
        match self.duplicate_ordinal {
            Some(ordinal) => format!("{} for {}#{ordinal}", trait_ref, self.target),
            None => format!("{} for {}", trait_ref, self.target),
        }
    }
}

/// Produces a session-independent implementation identity from syntax facts.
///
/// Each variable-length component is length-prefixed and option/duplicate
/// states use explicit domain bytes. This prevents concatenation ambiguity and
/// keeps identical duplicate declarations distinct without using allocation or
/// traversal addresses.
pub(super) fn stable_trait_impl_id(identity: &TraitImplIdentity) -> u64 {
    let mut hash = StableTraitImplHasher::new();
    hash.bytes(b"trait_impl");
    hash.string(&identity.target);
    hash.optional_string(identity.trait_ref.as_deref());
    hash.string_slice(&identity.generics);
    hash.u64(identity.where_clause.len() as u64);
    for (ty, bounds) in &identity.where_clause {
        hash.string(ty);
        hash.string_slice(bounds);
    }
    match identity.duplicate_ordinal {
        Some(ordinal) => {
            hash.bytes(b"duplicate");
            hash.u64(u64::from(ordinal));
        }
        None => hash.bytes(b"primary"),
    }
    hash.finish()
}

struct StableTraitImplHasher {
    value: u64,
}

impl StableTraitImplHasher {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    fn new() -> Self {
        Self {
            value: Self::OFFSET,
        }
    }

    fn finish(self) -> u64 {
        self.value
    }

    fn string_slice(&mut self, values: &[String]) {
        self.u64(values.len() as u64);
        for value in values {
            self.string(value);
        }
    }

    fn optional_string(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.bytes(b"some");
                self.string(value);
            }
            None => self.bytes(b"none"),
        }
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.value ^= u64::from(*byte);
            self.value = self.value.wrapping_mul(Self::PRIME);
        }
    }
}
