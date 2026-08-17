// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const FINGERPRINT_SET_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.llvm.codegen-unit-components.v2");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Independently attributable inputs to one object work-product fingerprint.
///
/// Keeping the components separate lets cache diagnostics distinguish policy
/// changes from definition, declaration-surface, and target changes.
pub struct CodegenUnitFingerprintComponents {
    /// Optimization and codegen policy fingerprint.
    pub policy: CodegenUnitFingerprint,
    /// Definitions emitted into the unit.
    pub definition: CodegenUnitFingerprint,
    /// Cross-unit declaration surface required by the unit.
    pub declarations: CodegenUnitFingerprint,
    /// Target machine, data layout, and toolchain identity.
    pub target: CodegenUnitFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Complete cache identity together with its attributable components.
pub struct CodegenUnitFingerprintSet {
    /// Domain-separated aggregate of all component fingerprints.
    pub fingerprint: CodegenUnitFingerprint,
    /// Component fingerprints retained for invalidation reporting.
    pub components: CodegenUnitFingerprintComponents,
}

impl CodegenUnitFingerprintSet {
    /// Builds the domain-separated aggregate in stable component order.
    pub fn new(components: CodegenUnitFingerprintComponents) -> Self {
        let mut builder = QueryFingerprintBuilder::new(FINGERPRINT_SET_DOMAIN);
        for component in [
            components.policy,
            components.definition,
            components.declarations,
            components.target,
        ] {
            for part in component.parts() {
                builder.write_u64(part);
            }
        }
        Self {
            fingerprint: CodegenUnitFingerprint::from_parts(builder.finish().parts()),
            components,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Component-wise reason that a cached object cannot be reused.
pub struct ObjectWorkProductInvalidation {
    /// Optimization or codegen policy changed.
    pub policy: bool,
    /// Definitions owned by the unit changed.
    pub definition: bool,
    /// Required declaration surface changed.
    pub declarations: bool,
    /// Target machine, layout, or toolchain changed.
    pub target: bool,
}

impl ObjectWorkProductInvalidation {
    /// Compares cached and expected component fingerprints.
    pub fn between(
        cached: CodegenUnitFingerprintComponents,
        expected: CodegenUnitFingerprintComponents,
    ) -> Self {
        Self {
            policy: cached.policy != expected.policy,
            definition: cached.definition != expected.definition,
            declarations: cached.declarations != expected.declarations,
            target: cached.target != expected.target,
        }
    }

    /// Returns the number of independently changed components.
    pub fn count(self) -> u32 {
        u32::from(self.policy)
            + u32::from(self.definition)
            + u32::from(self.declarations)
            + u32::from(self.target)
    }
}

#[derive(Debug, PartialEq, Eq)]
/// Result of looking up a native object work product.
pub enum ObjectWorkProductLookup {
    /// Exact fingerprint match with reusable object bytes.
    Hit(Vec<u8>),
    /// No cache entry exists for the stable unit key.
    NotFound,
    /// An entry exists but one or more fingerprint components changed.
    Invalidated(ObjectWorkProductInvalidation),
    /// The entry could not be decoded or failed integrity validation.
    Corrupt,
}

/// Persistent cache boundary for native object work products.
///
/// Implementations own storage synchronization and atomic publication. A cache
/// hit must correspond exactly to the supplied stable key and fingerprint set;
/// corrupt or stale bytes must never be returned as [`ObjectWorkProductLookup::Hit`].
pub trait ObjectWorkProductCache: Send + Sync {
    /// Loads and validates an object for `key` against `fingerprints`.
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<ObjectWorkProductLookup>;

    /// Atomically publishes verified object bytes for the exact fingerprint set.
    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()>;
}
