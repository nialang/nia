//! Trait implementation identity and candidate indexing.

use super::*;

/// Compact candidate index used by program-wide trait selection.
///
/// Values are indexes into the owning `ProgramTraitImplSignature` slice rather
/// than copied signatures, so the slice remains the only ordered source of
/// truth for implementation facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramTraitImplIndex {
    by_trait: HashMap<TraitId, Vec<usize>>,
}

impl ProgramTraitImplIndex {
    pub fn new(trait_impls: &[ProgramTraitImplSignature]) -> Self {
        let mut by_trait = HashMap::<TraitId, Vec<usize>>::new();
        for (index, impl_signature) in trait_impls.iter().enumerate() {
            by_trait
                .entry(impl_signature.trait_id)
                .or_default()
                .push(index);
        }
        Self { by_trait }
    }

    pub fn indexes_for_trait(&self, trait_id: TraitId) -> &[usize] {
        self.by_trait
            .get(&trait_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.by_trait.is_empty()
    }
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
