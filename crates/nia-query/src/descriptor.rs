// SPDX-License-Identifier: GPL-3.0-or-later
//! Declarative query contracts, fingerprint domains, and registry validation.

use super::*;
use std::io;

/// Declares the provider, storage, and change-detection contract for one query key.
///
/// Implementations must keep [`name`](Self::name) stable and unique within a
/// [`QueryRegistry`]. Fingerprints are persisted semantic identity: they must be
/// deterministic across processes and compiler revisions that share a domain.
pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    /// Value produced or published for this key.
    type Value: Send + Sync + 'static;

    /// Policy used to determine whether a recomputed value changed.
    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::None;
    /// Ownership policy used by the query slot.
    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::CacheOwnedArc;
    /// Source from which the slot obtains its value.
    const PROVIDER: QueryProviderPolicy = QueryProviderPolicy::KeyExecute;

    /// Stable registry and diagnostic name for this query kind.
    fn name() -> &'static str;
    /// Human-readable description of this specific key.
    fn description(&self) -> String {
        format!("{}::{self:?}", Self::name())
    }
    /// Computes this key's value when the provider policy is [`QueryProviderPolicy::KeyExecute`].
    fn execute_result(&self, db: &QueryDb<C>) -> QueryResult<Self::Value>;
    /// Returns the deterministic fingerprint required by [`QueryFingerprintPolicy::StableValue`].
    fn fingerprint(&self, _value: &Self::Value) -> Option<QueryFingerprint> {
        None
    }
    /// Tests semantic equality for [`QueryFingerprintPolicy::SemanticValue`].
    ///
    /// Returning `true` preserves the previous semantic fingerprint and keeps
    /// dependent queries green after this value is recomputed.
    fn values_equal(&self, _old: &Self::Value, _new: &Self::Value) -> bool {
        false
    }
}

/// Selects whether the key computes its value or receives it from an external producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProviderPolicy {
    /// Invoke [`QueryKey::execute_result`] when the slot needs a value.
    KeyExecute,
    /// Require a producer to transfer the value with [`QueryDb::publish_owned`].
    ExternallyPublished,
}

/// Selects how shared cached values participate in red/green validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFingerprintPolicy {
    /// Do not retain change identity for the value.
    None,
    /// Use the explicit deterministic fingerprint returned by [`QueryKey::fingerprint`].
    StableValue,
    /// Compare recomputed values with [`QueryKey::values_equal`].
    SemanticValue,
}

/// Deterministic 128-bit identity for a query value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueryFingerprint([u64; 2]);

impl QueryFingerprint {
    /// Constructs a fingerprint from its two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the two 64-bit lanes.
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

/// Versioned namespace that separates otherwise identical fingerprint inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FingerprintDomain(&'static str);

impl FingerprintDomain {
    /// Creates a domain named `nia.<segments>.vN`.
    ///
    /// Segments contain lowercase ASCII letters or digits and may be separated
    /// by `.` or `-`. Versions start at one and may not contain leading zeroes.
    /// This function panics in const evaluation when `domain` is invalid.
    pub const fn new(domain: &'static str) -> Self {
        assert!(
            valid_fingerprint_domain(domain),
            "invalid fingerprint domain"
        );
        Self(domain)
    }

    /// Returns the validated domain name.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

const fn valid_fingerprint_domain(domain: &str) -> bool {
    // Domains are persisted identity, not display text. Requiring a structured
    // `nia.<segments>.vN` spelling makes format changes explicit and prevents
    // accidental reuse of an unversioned hash namespace.
    let bytes = domain.as_bytes();
    if bytes.len() < 8
        || bytes[0] != b'n'
        || bytes[1] != b'i'
        || bytes[2] != b'a'
        || bytes[3] != b'.'
    {
        return false;
    }
    let mut version_start = bytes.len();
    while version_start > 0 && bytes[version_start - 1].is_ascii_digit() {
        version_start -= 1;
    }
    if version_start < 2
        || version_start == bytes.len()
        || bytes[version_start - 2] != b'.'
        || bytes[version_start - 1] != b'v'
        || bytes[version_start] == b'0'
    {
        return false;
    }
    let domain_end = version_start - 2;
    if domain_end <= 4 {
        return false;
    }
    let mut index = 4;
    let mut requires_alphanumeric = true;
    while index < domain_end {
        let byte = bytes[index];
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            requires_alphanumeric = false;
        } else if byte == b'.' || byte == b'-' {
            if requires_alphanumeric {
                return false;
            }
            requires_alphanumeric = true;
        } else {
            return false;
        }
        index += 1;
    }
    !requires_alphanumeric
}

/// Incrementally constructs a deterministic, domain-separated fingerprint.
pub struct QueryFingerprintBuilder {
    state: [u64; 2],
}

/// Incremental writer for one length-prefixed byte field in a fingerprint.
///
/// Call [`finish`](Self::finish) after writing exactly the declared number of
/// bytes. If writing or finishing fails, discard the parent fingerprint builder.
#[must_use = "the byte stream must be completed with `finish`"]
pub struct QueryFingerprintBytesWriter<'a> {
    builder: &'a mut QueryFingerprintBuilder,
    remaining: u64,
}

impl QueryFingerprintBuilder {
    const FIRST_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FIRST_PRIME: u64 = 0x0000_0100_0000_01b3;
    const SECOND_OFFSET: u64 = 0x6c62_272e_07bb_0142;
    const SECOND_PRIME: u64 = 0x9e37_79b1_85eb_ca87;

    /// Starts a fingerprint in `domain`.
    pub fn new(domain: FingerprintDomain) -> Self {
        let mut builder = Self {
            state: [Self::FIRST_OFFSET, Self::SECOND_OFFSET],
        };
        builder.write_str(domain.as_str());
        builder
    }

    /// Appends one byte.
    pub fn write_u8(&mut self, value: u8) {
        self.write_raw_bytes(&[value]);
    }

    /// Appends one little-endian 64-bit integer.
    pub fn write_u64(&mut self, value: u64) {
        self.write_raw_bytes(&value.to_le_bytes());
    }

    /// Appends a length-prefixed byte field.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u64(bytes.len() as u64);
        self.write_raw_bytes(bytes);
    }

    /// Begins a streaming equivalent of [`write_bytes`](Self::write_bytes).
    ///
    /// This keeps fingerprints stable for payloads that are too large to
    /// materialize as one slice. The declared length is part of the fingerprint.
    pub fn bytes_writer(&mut self, length: u64) -> QueryFingerprintBytesWriter<'_> {
        self.write_u64(length);
        QueryFingerprintBytesWriter {
            builder: self,
            remaining: length,
        }
    }

    /// Appends a UTF-8 string as a length-prefixed byte field.
    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    /// Appends both lanes of an existing fingerprint.
    pub fn write_fingerprint(&mut self, fingerprint: QueryFingerprint) {
        for part in fingerprint.parts() {
            self.write_u64(part);
        }
    }

    /// Finishes and returns the 128-bit fingerprint.
    pub fn finish(self) -> QueryFingerprint {
        QueryFingerprint(self.state)
    }

    fn write_raw_bytes(&mut self, bytes: &[u8]) {
        // Two independently mixed lanes make the persisted fingerprint wider
        // without relying on platform hashers or randomized process state.
        for byte in bytes {
            self.state[0] ^= u64::from(*byte);
            self.state[0] = self.state[0].wrapping_mul(Self::FIRST_PRIME);
            self.state[1] ^= u64::from(*byte);
            self.state[1] = self.state[1]
                .rotate_left(7)
                .wrapping_mul(Self::SECOND_PRIME);
        }
    }
}

impl QueryFingerprintBytesWriter<'_> {
    /// Adds the next contiguous payload chunk.
    pub fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        let length = u64::try_from(bytes.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "fingerprint chunk length does not fit in u64",
            )
        })?;
        if length > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fingerprint byte stream exceeds its declared length",
            ));
        }
        self.builder.write_raw_bytes(bytes);
        self.remaining -= length;
        Ok(())
    }

    /// Completes the field after verifying that every declared byte was written.
    pub fn finish(self) -> io::Result<()> {
        if self.remaining == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fingerprint byte stream ended before its declared length",
            ))
        }
    }
}

/// Selects whether a query value is retained or transferred to one consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStoragePolicy {
    /// Retain an [`Arc`] in the cache and share it with every caller.
    CacheOwnedArc,
    /// Move the value out of its slot; no value fingerprint can be retained.
    SingleConsumerOwned,
}

/// Runtime-readable contract registered for one query key type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDescriptor {
    /// Stable query name.
    pub name: &'static str,
    /// Fully qualified context type name.
    pub context_type: &'static str,
    /// Fully qualified key type name.
    pub key_type: &'static str,
    /// Fully qualified value type name.
    pub value_type: &'static str,
    /// Value provider policy.
    pub provider: QueryProviderPolicy,
    /// Change-detection policy.
    pub fingerprint: QueryFingerprintPolicy,
    /// Cached value ownership policy.
    pub storage: QueryStoragePolicy,
}

/// Declarative set of query key types accepted by a registered database.
#[derive(Debug, Default)]
pub struct QueryRegistry {
    descriptors: FastHashMap<TypeId, QueryDescriptor>,
    names: FastHashMap<&'static str, TypeId>,
}

impl QueryRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `K` and validates its provider, storage, and fingerprint policies.
    ///
    /// Registration panics for duplicate key types or names, for fingerprinted
    /// single-consumer values, and for externally published values that are not
    /// un-fingerprinted single-consumer payloads.
    pub fn register<C, K>(&mut self)
    where
        C: 'static,
        K: QueryKey<C>,
    {
        // Single-consumer payloads move out of storage, so there is no retained
        // value from which a stable fingerprint could later be recovered.
        assert!(
            K::STORAGE == QueryStoragePolicy::CacheOwnedArc
                || K::FINGERPRINT == QueryFingerprintPolicy::None,
            "single-consumer query `{}` cannot retain a value fingerprint",
            K::name()
        );
        // External publishers need transfer semantics: a cache-owned value
        // could otherwise outlive and obscure the producer's retirement edge.
        assert!(
            K::PROVIDER == QueryProviderPolicy::KeyExecute
                || (K::STORAGE == QueryStoragePolicy::SingleConsumerOwned
                    && K::FINGERPRINT == QueryFingerprintPolicy::None),
            "externally published query `{}` must use single-consumer owned storage",
            K::name()
        );
        let key_type_id = TypeId::of::<K>();
        assert!(
            !self.descriptors.contains_key(&key_type_id),
            "query key type `{}` is already registered",
            std::any::type_name::<K>()
        );
        if let Some(existing) = self.names.get(K::name()) {
            let existing = self
                .descriptors
                .get(existing)
                .expect("query registry name index must reference a descriptor");
            panic!(
                "query name `{}` is already registered for `{}`",
                K::name(),
                existing.key_type
            );
        }
        self.names.insert(K::name(), key_type_id);
        self.descriptors.insert(
            key_type_id,
            QueryDescriptor {
                name: K::name(),
                context_type: std::any::type_name::<C>(),
                key_type: std::any::type_name::<K>(),
                value_type: std::any::type_name::<K::Value>(),
                provider: K::PROVIDER,
                fingerprint: K::FINGERPRINT,
                storage: K::STORAGE,
            },
        );
    }

    /// Returns registered descriptors sorted by query name.
    pub fn descriptors(&self) -> Vec<QueryDescriptor> {
        let mut descriptors = self.descriptors.values().cloned().collect::<Vec<_>>();
        descriptors.sort_by_key(|descriptor| descriptor.name);
        descriptors
    }

    pub(super) fn assert_registered<C, K>(&self)
    where
        K: QueryKey<C>,
    {
        assert!(
            self.descriptors.contains_key(&TypeId::of::<K>()),
            "query key type `{}` is not in the declarative registry",
            std::any::type_name::<K>()
        );
    }
}
