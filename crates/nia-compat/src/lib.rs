// SPDX-License-Identifier: GPL-3.0-or-later
//! Central compatibility registry for compiler ABIs and persisted data.
//!
//! Format magic identifies the payload kind. Release compatibility identifies
//! the complete compiler/toolchain contract; payloads do not carry independent
//! hand-bumped schema counters.

use std::fmt::Write;

/// Version of the compiler crate set used in toolchain compatibility checks.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Release-scoped compatibility epoch shared by every persisted compiler
/// product and ABI boundary. The epoch is derived from the public release
/// line (`0.1.x` => 1, `0.2.x` => 2, `1.0.x` => 100) rather than from the
/// number of implementation changes made during development.
pub const RELEASE_COMPATIBILITY: u32 = release_compatibility(COMPILER_VERSION);

const fn decimal_component(bytes: &[u8], start: usize, end: usize) -> u32 {
    let mut value = 0;
    let mut index = start;
    while index < end {
        value = value * 10 + (bytes[index] - b'0') as u32;
        index += 1;
    }
    value
}

const fn release_compatibility(version: &str) -> u32 {
    let bytes = version.as_bytes();
    let mut first_dot = 0;
    while first_dot < bytes.len() && bytes[first_dot] != b'.' {
        first_dot += 1;
    }
    let mut second_dot = first_dot + 1;
    while second_dot < bytes.len() && bytes[second_dot] != b'.' {
        second_dot += 1;
    }
    let major = decimal_component(bytes, 0, first_dot);
    let minor = decimal_component(bytes, first_dot + 1, second_dot);
    if major == 0 {
        minor
    } else {
        major * 1000 + minor
    }
}

/// Release-scoped identities for generated ABI boundaries.
pub mod abi {
    /// Symbol mangling compatibility epoch.
    pub const MANGLE: u64 = super::RELEASE_COMPATIBILITY as u64;
    /// LLVM code-generation compatibility epoch.
    pub const LLVM_CODEGEN: u64 = super::RELEASE_COMPATIBILITY as u64;
}

/// Versions coupling the compiler to installed resources and build runners.
pub mod toolchain {
    /// Installed toolchain resource compatibility epoch.
    pub const RESOURCE_LAYOUT: u32 = super::RELEASE_COMPATIBILITY;
    /// Standard-library compatibility epoch.
    pub const STANDARD_LIBRARY: u32 = super::RELEASE_COMPATIBILITY;
    /// Build protocol compatibility epoch shared with compiled runners.
    pub const BUILD_PROTOCOL: u32 = super::RELEASE_COMPATIBILITY;
}

/// Binary payload identity consisting of a diagnostic name, magic, and the
/// release compatibility epoch. The third constructor argument is retained
/// only at the call sites during this migration and is intentionally ignored;
/// all payloads share the same release-derived value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedFormat {
    /// Human-readable registry identity.
    pub name: &'static str,
    /// Exact eight-byte prefix distinguishing this payload kind and generation.
    pub magic: &'static [u8; 8],
    /// Release compatibility epoch.
    pub schema: u32,
}

impl PersistedFormat {
    /// Defines one persisted payload contract.
    pub const fn new(name: &'static str, magic: &'static [u8; 8]) -> Self {
        Self {
            name,
            magic,
            schema: RELEASE_COMPATIBILITY,
        }
    }
}

/// Release-scoped directory namespace isolating incompatible persisted entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedNamespace {
    /// Human-readable registry identity.
    pub name: &'static str,
    /// Release compatibility epoch.
    pub schema: u32,
    /// Directory component for this release line.
    pub path_component: &'static str,
}

impl PersistedNamespace {
    /// Defines one versioned persistence namespace.
    pub const fn new(name: &'static str, path_component: &'static str) -> Self {
        Self {
            name,
            schema: RELEASE_COMPATIBILITY,
            path_component,
        }
    }
}

/// Authoritative registry of persisted payloads and cache namespaces.
pub mod formats {
    use super::{PersistedFormat, PersistedNamespace};

    /// Immutable action graph handed from a build runner to the coordinator.
    pub const BUILD_PLAN: PersistedFormat = PersistedFormat::new("build-plan", b"NIA-PLN\0");
    /// Invocation-private configuration consumed by a compiled build runner.
    pub const RUNNER_CONFIG: PersistedFormat = PersistedFormat::new("runner-config", b"NIARUNCF");

    /// Shared namespace for persistent frontend products.
    pub const FRONTEND_CACHE: PersistedNamespace = PersistedNamespace::new("frontend-cache", "v2");
    /// Source dependency manifest for validating a frontend cache entry.
    pub const FRONTEND_DEPENDENCY_MANIFEST: PersistedFormat =
        PersistedFormat::new("frontend-dependency-manifest", b"NIAFDM01");
    /// Persisted facade and reexport facts for one module.
    pub const FRONTEND_FACADE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-facade-facts", b"NIAFFF01");
    /// Persisted module dependency edges.
    pub const FRONTEND_MODULE_DEPENDENCIES: PersistedFormat =
        PersistedFormat::new("frontend-module-dependencies", b"NIAFMD01");
    /// Persisted provider candidate summary.
    pub const FRONTEND_PROVIDER_SUMMARY: PersistedFormat =
        PersistedFormat::new("frontend-provider-summary", b"NIAFPS01");
    /// Persisted fixed-point provider demand plan.
    pub const FRONTEND_PROVIDER_DEMAND_PLAN: PersistedFormat =
        PersistedFormat::new("frontend-provider-demand-plan", b"NIAFPD01");
    /// Persisted module public-surface facts.
    pub const FRONTEND_PUBLIC_SURFACE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-public-surface-facts", b"NIAFPF01");

    /// Persisted type-name resolution for signatures.
    pub const SIGNATURE_TYPE_RESOLUTION: PersistedFormat =
        PersistedFormat::new("signature-type-resolution", b"NIASR001");
    /// Persisted lowered signature types.
    pub const SIGNATURE_TYPE_LOWERING: PersistedFormat =
        PersistedFormat::new("signature-type-lowering", b"NIASL001");
    /// Persisted item signature collection.
    pub const SIGNATURE_ITEM_SIGNATURES: PersistedFormat =
        PersistedFormat::new("signature-item-signatures", b"NIASI001");
    /// Persisted extension-validation diagnostic set.
    pub const EXTENSION_VALIDATION_DIAGNOSTICS: PersistedFormat =
        PersistedFormat::new("extension-validation-diagnostics", b"NIAEV001");
    /// Persisted executable value-reference reachability edges.
    pub const EXECUTABLE_VALUE_REF_EDGES: PersistedFormat =
        PersistedFormat::new("executable-value-ref-edges", b"NIAER001");
    /// Certificate proving that cached frontend checks covered their inputs.
    pub const CHECK_CERTIFICATE: PersistedFormat =
        PersistedFormat::new("check-certificate", b"NIACC001");
    /// Path-independent stable diagnostic bundle for one module.
    pub const STABLE_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-diagnostic-bundle", b"NIADB001");
    /// Path-independent stable diagnostic bundle for a program.
    pub const STABLE_PROGRAM_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-program-diagnostic-bundle", b"NIAPD001");

    /// Build-action cache namespace for generated files.
    pub const GENERATED_FILE_CACHE: PersistedNamespace =
        PersistedNamespace::new("generated-file-cache", "v2");
    /// Cached generated-file action result.
    pub const GENERATED_FILE_ENTRY: PersistedFormat =
        PersistedFormat::new("generated-file-entry", b"NIAGEN01");
    /// Build-action cache namespace for external commands.
    pub const EXTERNAL_COMMAND_CACHE: PersistedNamespace =
        PersistedNamespace::new("external-command-cache", "v2");
    /// Cached external-command action result.
    pub const EXTERNAL_COMMAND_ENTRY: PersistedFormat =
        PersistedFormat::new("external-command-entry", b"NIACMD01");
    /// Build-action cache namespace for compiler checks.
    pub const COMPILER_CHECK_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-check-cache", "v2");
    /// Cached compiler-check action result.
    pub const COMPILER_CHECK_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-check-entry", b"NIACKC01");
    /// Build-action cache namespace for compiler emission.
    pub const COMPILER_EMIT_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-emit-cache", "v2");
    /// Cached compiler-emission action result.
    pub const COMPILER_EMIT_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-emit-entry", b"NIAKCE01");

    /// Namespace for recoverable output publication transactions.
    pub const OUTPUT_TRANSACTION: PersistedNamespace =
        PersistedNamespace::new("output-transaction", "v2");
    /// Journal recording output transaction state and recovery intent.
    pub const OUTPUT_TRANSACTION_JOURNAL: PersistedFormat =
        PersistedFormat::new("output-transaction-journal", b"NIATXN01");
    /// Metadata for one prepared output awaiting publication.
    pub const OUTPUT_TRANSACTION_PREPARED: PersistedFormat =
        PersistedFormat::new("output-transaction-prepared", b"NIAPRP01");

    /// Driver namespace for incremental object work products.
    pub const OBJECT_WORK_PRODUCT_CACHE: PersistedNamespace =
        PersistedNamespace::new("object-work-product-cache", "v2");
    /// Persisted object work product and validation metadata.
    pub const OBJECT_WORK_PRODUCT: PersistedFormat =
        PersistedFormat::new("object-work-product", b"NIAOBJ01");
    /// Driver namespace for executable link results.
    pub const LINK_RESULT_CACHE: PersistedNamespace =
        PersistedNamespace::new("link-result-cache", "v2");
    /// Persisted executable link result and component fingerprints.
    pub const LINK_RESULT: PersistedFormat = PersistedFormat::new("link-result", b"NIALNK01");
    /// Driver namespace for static archive results.
    pub const ARCHIVE_RESULT_CACHE: PersistedNamespace =
        PersistedNamespace::new("archive-result-cache", "v2");
    /// Persisted static archive result and component fingerprints.
    pub const ARCHIVE_RESULT: PersistedFormat = PersistedFormat::new("archive-result", b"NIAARC01");
    /// Compiled package metadata container.
    pub const PACKAGE_METADATA: PersistedFormat =
        PersistedFormat::new("package-metadata", b"NIAPKG01");

    /// Complete payload registry used for uniqueness checks and audits.
    pub const ALL: &[PersistedFormat] = &[
        BUILD_PLAN,
        RUNNER_CONFIG,
        FRONTEND_DEPENDENCY_MANIFEST,
        FRONTEND_FACADE_FACTS,
        FRONTEND_MODULE_DEPENDENCIES,
        FRONTEND_PROVIDER_SUMMARY,
        FRONTEND_PROVIDER_DEMAND_PLAN,
        FRONTEND_PUBLIC_SURFACE_FACTS,
        SIGNATURE_TYPE_RESOLUTION,
        SIGNATURE_TYPE_LOWERING,
        SIGNATURE_ITEM_SIGNATURES,
        EXTENSION_VALIDATION_DIAGNOSTICS,
        EXECUTABLE_VALUE_REF_EDGES,
        CHECK_CERTIFICATE,
        STABLE_DIAGNOSTIC_BUNDLE,
        STABLE_PROGRAM_DIAGNOSTIC_BUNDLE,
        GENERATED_FILE_ENTRY,
        EXTERNAL_COMMAND_ENTRY,
        COMPILER_CHECK_ENTRY,
        COMPILER_EMIT_ENTRY,
        OUTPUT_TRANSACTION_JOURNAL,
        OUTPUT_TRANSACTION_PREPARED,
        OBJECT_WORK_PRODUCT,
        LINK_RESULT,
        ARCHIVE_RESULT,
        PACKAGE_METADATA,
    ];

    /// Complete cache namespace registry used for version-path checks and audits.
    pub const NAMESPACES: &[PersistedNamespace] = &[
        FRONTEND_CACHE,
        GENERATED_FILE_CACHE,
        EXTERNAL_COMMAND_CACHE,
        COMPILER_CHECK_CACHE,
        COMPILER_EMIT_CACHE,
        OUTPUT_TRANSACTION,
        OBJECT_WORK_PRODUCT_CACHE,
        LINK_RESULT_CACHE,
        ARCHIVE_RESULT_CACHE,
    ];
}

/// Renders the canonical installed-resource manifest for this compiler build.
pub fn toolchain_manifest() -> String {
    let mut manifest = String::new();
    writeln!(manifest, "# Nia toolchain resource compatibility identity.").unwrap();
    writeln!(manifest, "compiler-version={COMPILER_VERSION}").unwrap();
    writeln!(manifest, "release-compatibility={RELEASE_COMPATIBILITY}").unwrap();
    manifest
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use super::{formats, toolchain_manifest};

    #[test]
    fn release_compatibility_follows_public_release_line() {
        assert_eq!(super::release_compatibility("0.1.0"), 1);
        assert_eq!(super::release_compatibility("0.2.0-dev"), 2);
        assert_eq!(super::release_compatibility("1.0.0"), 1000);
        assert_eq!(super::release_compatibility("2.3.7"), 2003);
    }

    #[test]
    fn persisted_identity_names_and_magics_are_unique() {
        let mut names = BTreeSet::new();
        let mut magics = BTreeSet::new();
        for format in formats::ALL {
            assert!(format.schema > 0, "{} has no schema", format.name);
            assert!(
                names.insert(format.name),
                "duplicate format name {}",
                format.name
            );
            assert!(
                magics.insert(format.magic),
                "duplicate magic for {}",
                format.name
            );
        }
    }

    #[test]
    fn namespace_paths_match_their_schema() {
        let mut names = BTreeSet::new();
        for namespace in formats::NAMESPACES {
            assert!(
                names.insert(namespace.name),
                "duplicate namespace name {}",
                namespace.name
            );
            assert_eq!(namespace.path_component, "v2");
        }
    }

    #[test]
    fn checked_in_toolchain_manifest_is_generated_from_the_registry() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lib/toolchain.meta");
        let checked_in = fs::read_to_string(&path).expect("read checked-in toolchain manifest");
        assert_eq!(
            checked_in,
            toolchain_manifest(),
            "run `cargo run -p nia-compat -- write lib/toolchain.meta`"
        );
    }
}
