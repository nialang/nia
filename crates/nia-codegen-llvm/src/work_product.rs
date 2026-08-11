// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const FINGERPRINT_SET_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.llvm.codegen-unit-components.v2");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodegenUnitFingerprintComponents {
    pub policy: CodegenUnitFingerprint,
    pub definition: CodegenUnitFingerprint,
    pub declarations: CodegenUnitFingerprint,
    pub target: CodegenUnitFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodegenUnitFingerprintSet {
    pub fingerprint: CodegenUnitFingerprint,
    pub components: CodegenUnitFingerprintComponents,
}

impl CodegenUnitFingerprintSet {
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
pub struct ObjectWorkProductInvalidation {
    pub policy: bool,
    pub definition: bool,
    pub declarations: bool,
    pub target: bool,
}

impl ObjectWorkProductInvalidation {
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

    pub fn count(self) -> u32 {
        u32::from(self.policy)
            + u32::from(self.definition)
            + u32::from(self.declarations)
            + u32::from(self.target)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ObjectWorkProductLookup {
    Hit(Vec<u8>),
    NotFound,
    Invalidated(ObjectWorkProductInvalidation),
    Corrupt,
}

pub trait ObjectWorkProductCache: Send + Sync {
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<ObjectWorkProductLookup>;

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()>;
}
