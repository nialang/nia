// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt::Write;

pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod abi {
    pub const MANGLE: u64 = 3;
    pub const LLVM_CODEGEN: u64 = 1;
}

pub mod toolchain {
    pub const RESOURCE_LAYOUT: u32 = 1;
    pub const STANDARD_LIBRARY: u32 = 1;
    pub const BUILD_PROTOCOL: u32 = 10;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedFormat {
    pub name: &'static str,
    pub magic: &'static [u8; 8],
    pub schema: u32,
}

impl PersistedFormat {
    pub const fn new(name: &'static str, magic: &'static [u8; 8], schema: u32) -> Self {
        Self {
            name,
            magic,
            schema,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedNamespace {
    pub name: &'static str,
    pub schema: u32,
    pub path_component: &'static str,
}

impl PersistedNamespace {
    pub const fn new(name: &'static str, schema: u32, path_component: &'static str) -> Self {
        Self {
            name,
            schema,
            path_component,
        }
    }
}

pub mod formats {
    use super::{PersistedFormat, PersistedNamespace};

    pub const BUILD_PLAN: PersistedFormat =
        PersistedFormat::new("build-plan", b"NIA-PLN\0", super::toolchain::BUILD_PROTOCOL);
    pub const RUNNER_CONFIG: PersistedFormat =
        PersistedFormat::new("runner-config", b"NIARUNCF", 1);

    pub const FRONTEND_CACHE: PersistedNamespace =
        PersistedNamespace::new("frontend-cache", 4, "v4");
    pub const FRONTEND_DEPENDENCY_MANIFEST: PersistedFormat =
        PersistedFormat::new("frontend-dependency-manifest", b"NIAFDM02", 2);
    pub const FRONTEND_FACADE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-facade-facts", b"NIAFFF02", 2);
    pub const FRONTEND_MODULE_DEPENDENCIES: PersistedFormat =
        PersistedFormat::new("frontend-module-dependencies", b"NIAFMD03", 3);
    pub const FRONTEND_PROVIDER_SUMMARY: PersistedFormat =
        PersistedFormat::new("frontend-provider-summary", b"NIAFPS03", 3);
    pub const FRONTEND_PROVIDER_DEMAND_PLAN: PersistedFormat =
        PersistedFormat::new("frontend-provider-demand-plan", b"NIAFPD03", 3);
    pub const FRONTEND_PUBLIC_SURFACE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-public-surface-facts", b"NIAFPF01", 1);

    pub const SIGNATURE_TYPE_RESOLUTION: PersistedFormat =
        PersistedFormat::new("signature-type-resolution", b"NIASR003", 3);
    pub const SIGNATURE_TYPE_LOWERING: PersistedFormat =
        PersistedFormat::new("signature-type-lowering", b"NIASL003", 3);
    pub const SIGNATURE_ITEM_SIGNATURES: PersistedFormat =
        PersistedFormat::new("signature-item-signatures", b"NIASI008", 8);
    pub const EXTENSION_VALIDATION_DIAGNOSTICS: PersistedFormat =
        PersistedFormat::new("extension-validation-diagnostics", b"NIAEV002", 2);
    pub const EXECUTABLE_VALUE_REF_EDGES: PersistedFormat =
        PersistedFormat::new("executable-value-ref-edges", b"NIAER001", 1);
    pub const CHECK_CERTIFICATE: PersistedFormat =
        PersistedFormat::new("check-certificate", b"NIACC002", 2);
    pub const STABLE_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-diagnostic-bundle", b"NIADB001", 1);
    pub const STABLE_PROGRAM_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-program-diagnostic-bundle", b"NIAPD001", 1);

    pub const GENERATED_FILE_CACHE: PersistedNamespace =
        PersistedNamespace::new("generated-file-cache", 1, "v1");
    pub const GENERATED_FILE_ENTRY: PersistedFormat =
        PersistedFormat::new("generated-file-entry", b"NIAGEN01", 1);
    pub const EXTERNAL_COMMAND_CACHE: PersistedNamespace =
        PersistedNamespace::new("external-command-cache", 3, "v3");
    pub const EXTERNAL_COMMAND_ENTRY: PersistedFormat =
        PersistedFormat::new("external-command-entry", b"NIACMD03", 3);
    pub const COMPILER_CHECK_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-check-cache", 2, "v2");
    pub const COMPILER_CHECK_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-check-entry", b"NIACKC02", 2);
    pub const COMPILER_EMIT_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-emit-cache", 3, "v3");
    pub const COMPILER_EMIT_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-emit-entry", b"NIAKCE03", 3);

    pub const OUTPUT_TRANSACTION: PersistedNamespace =
        PersistedNamespace::new("output-transaction", 2, "v2");
    pub const OUTPUT_TRANSACTION_JOURNAL: PersistedFormat =
        PersistedFormat::new("output-transaction-journal", b"NIATXN02", 2);
    pub const OUTPUT_TRANSACTION_PREPARED: PersistedFormat =
        PersistedFormat::new("output-transaction-prepared", b"NIAPRP01", 1);

    pub const OBJECT_WORK_PRODUCT_CACHE: PersistedNamespace =
        PersistedNamespace::new("object-work-product-cache", 2, "v2");
    pub const OBJECT_WORK_PRODUCT: PersistedFormat =
        PersistedFormat::new("object-work-product", b"NIAOBJ02", 2);
    pub const LINK_RESULT_CACHE: PersistedNamespace =
        PersistedNamespace::new("link-result-cache", 3, "v3");
    pub const LINK_RESULT: PersistedFormat = PersistedFormat::new("link-result", b"NIALNK03", 3);
    pub const ARCHIVE_RESULT_CACHE: PersistedNamespace =
        PersistedNamespace::new("archive-result-cache", 1, "v1");
    pub const ARCHIVE_RESULT: PersistedFormat =
        PersistedFormat::new("archive-result", b"NIAARC01", 1);

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
    ];

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

pub fn toolchain_manifest() -> String {
    let mut manifest = String::new();
    writeln!(manifest, "# Nia toolchain resource compatibility identity.").unwrap();
    writeln!(
        manifest,
        "resource-layout-schema={}",
        toolchain::RESOURCE_LAYOUT
    )
    .unwrap();
    writeln!(manifest, "compiler-version={COMPILER_VERSION}").unwrap();
    writeln!(manifest, "std-schema={}", toolchain::STANDARD_LIBRARY).unwrap();
    writeln!(
        manifest,
        "build-protocol-schema={}",
        toolchain::BUILD_PROTOCOL
    )
    .unwrap();
    manifest
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use super::{formats, toolchain_manifest};

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
            assert_eq!(
                namespace.path_component,
                format!("v{}", namespace.schema),
                "namespace {} does not match its schema",
                namespace.name
            );
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
