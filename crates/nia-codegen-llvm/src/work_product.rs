// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkProductReuse {
    Hit,
    Miss {
        reason: WorkProductReuseMiss,
        write_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkProductReuseMiss {
    Disabled,
    NotFound,
    Invalidated(CodegenWorkProductInvalidation),
    Corrupt,
    ReadError,
}

#[derive(Debug, Default)]
pub(super) struct WorkProductReuseCounts {
    hits: u64,
    disabled: u64,
    not_found: u64,
    invalidated: u64,
    invalidated_policy: u64,
    invalidated_definition: u64,
    invalidated_declarations: u64,
    invalidated_target: u64,
    corrupt: u64,
    read_error: u64,
    write_error: u64,
}

impl WorkProductReuseCounts {
    pub(super) fn record(&mut self, reuse: WorkProductReuse) {
        match reuse {
            WorkProductReuse::Hit => self.hits += 1,
            WorkProductReuse::Miss {
                reason,
                write_error,
            } => {
                self.write_error += u64::from(write_error);
                match reason {
                    WorkProductReuseMiss::Disabled => self.disabled += 1,
                    WorkProductReuseMiss::NotFound => self.not_found += 1,
                    WorkProductReuseMiss::Invalidated(reasons) => {
                        self.invalidated += 1;
                        self.invalidated_policy += u64::from(reasons.policy);
                        self.invalidated_definition += u64::from(reasons.definition);
                        self.invalidated_declarations += u64::from(reasons.declarations);
                        self.invalidated_target += u64::from(reasons.target);
                    }
                    WorkProductReuseMiss::Corrupt => self.corrupt += 1,
                    WorkProductReuseMiss::ReadError => self.read_error += 1,
                }
            }
        }
    }

    pub(super) fn emit(&self, product: &str) {
        let misses =
            self.disabled + self.not_found + self.invalidated + self.corrupt + self.read_error;
        let counter = |suffix| format!("llvm.{product}_{suffix}");
        nia_timing::emit_counter(counter("reuse_hits"), self.hits);
        nia_timing::emit_counter(counter("reuse_misses"), misses);
        nia_timing::emit_counter(counter("reuse_miss_disabled"), self.disabled);
        nia_timing::emit_counter(counter("reuse_miss_not_found"), self.not_found);
        nia_timing::emit_counter(counter("reuse_miss_invalidated"), self.invalidated);
        nia_timing::emit_counter(counter("invalidation_policy"), self.invalidated_policy);
        nia_timing::emit_counter(
            counter("invalidation_definition"),
            self.invalidated_definition,
        );
        nia_timing::emit_counter(
            counter("invalidation_declarations"),
            self.invalidated_declarations,
        );
        nia_timing::emit_counter(counter("invalidation_target"), self.invalidated_target);
        nia_timing::emit_counter(counter("reuse_miss_corrupt"), self.corrupt);
        nia_timing::emit_counter(counter("reuse_miss_read_error"), self.read_error);
        nia_timing::emit_counter(counter("reuse_write_errors"), self.write_error);
    }
}

const FINGERPRINT_SET_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.llvm.codegen-unit-components");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Independently attributable inputs to one codegen work-product fingerprint.
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
/// Component-wise reason that a cached codegen product cannot be reused.
pub struct CodegenWorkProductInvalidation {
    /// Optimization or codegen policy changed.
    pub policy: bool,
    /// Definitions owned by the unit changed.
    pub definition: bool,
    /// Required declaration surface changed.
    pub declarations: bool,
    /// Target machine, layout, or toolchain changed.
    pub target: bool,
}

impl CodegenWorkProductInvalidation {
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
/// Result of looking up a codegen work product.
pub enum CodegenWorkProductLookup {
    /// Exact fingerprint match with reusable payload bytes.
    Hit(Vec<u8>),
    /// No cache entry exists for the stable unit key.
    NotFound,
    /// An entry exists but one or more fingerprint components changed.
    Invalidated(CodegenWorkProductInvalidation),
    /// The entry could not be decoded or failed integrity validation.
    Corrupt,
}

/// Persistent cache boundary for native object work products.
///
/// Implementations own storage synchronization and atomic publication. A cache
/// hit must correspond exactly to the supplied stable key and fingerprint set;
/// corrupt or stale bytes must never be returned as [`CodegenWorkProductLookup::Hit`].
pub trait ObjectWorkProductCache: Send + Sync {
    /// Loads and validates an object for `key` against `fingerprints`.
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup>;

    /// Atomically publishes verified object bytes for the exact fingerprint set.
    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()>;
}

/// Persistent cache boundary for LTO pre-link bitcode work products.
///
/// Implementations must keep ThinLTO and full-LTO formats distinct and own
/// storage synchronization plus atomic publication. A hit must exactly match
/// the requested mode, stable key, and fingerprint set.
pub trait LtoModuleWorkProductCache: Send + Sync {
    /// Loads and validates pre-link bitcode for `key` and `mode`.
    fn load(
        &self,
        mode: crate::LtoMode,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup>;

    /// Atomically publishes verified pre-link bitcode for the exact identity.
    fn publish(
        &self,
        mode: crate::LtoMode,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()>;
}
