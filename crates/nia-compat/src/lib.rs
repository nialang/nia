// SPDX-License-Identifier: GPL-3.0-or-later
//! Current protocol registry for compiler ABIs and persisted data.
//!
//! Format magic identifies the payload kind. The release-derived protocol
//! epoch identifies the compiler/toolchain compatibility line; development
//! changes and individual payloads do not carry hand-bumped schema counters.

/// Version of the compiler crate set used in toolchain compatibility checks.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Replaces a filesystem destination with a fully written sibling path.
/// Windows' `rename` does not replace an existing destination, so all cache
/// and publication owners use this boundary instead of relying on host
/// rename behavior.
pub fn replace_path(
    source: impl AsRef<std::path::Path>,
    destination: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    #[cfg(not(windows))]
    {
        std::fs::rename(source, destination)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                windows_sys::Win32::Storage::FileSystem::REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        let ok = if ok == 0 {
            unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                        | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
                )
            }
        } else {
            ok
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Creates a directory path without following a symbolic link or reparse
/// point in any existing or newly-created component.
///
/// A relative path whose missing components reach its start is anchored at the
/// current directory, so `build/out` creates `build` beside the caller. A
/// missing `.` or `..` component cannot be created and reports `NotFound`.
pub fn create_real_directory(path: &std::path::Path) -> std::io::Result<()> {
    use std::{fs, io, path::Path};

    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    let existing = loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) => break (current.clone(), metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                current = match current.parent() {
                    Some(parent) if parent.as_os_str().is_empty() => Path::new(".").to_path_buf(),
                    Some(parent) => parent.to_path_buf(),
                    None => return Err(error),
                };
            }
            Err(error) => return Err(error),
        }
    };
    validate_real_directory(&existing.0, &existing.1)?;
    for name in missing.iter().rev() {
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(&current)?;
        validate_real_directory(&current, &metadata)?;
    }
    Ok(())
}

/// Reports whether metadata describes a symbolic link or a Windows reparse
/// point (symbolic links, junctions, and other redirecting entries).
pub fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Resolves a root-owned symbolic link directly under `/`, such as macOS's
/// `/var -> private/var` or a merged-usr `/lib -> usr/lib`.
///
/// Link-rejecting path walks follow only these: replacing an entry in `/`
/// requires root, so such a link cannot redirect a walk any more than root
/// already can. Every other link stays rejected. Returns `None` when `name`
/// is not such a link, and on hosts without a single filesystem root.
pub fn trusted_root_link_target(name: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let link = std::path::Path::new("/").join(name);
        let metadata = std::fs::symlink_metadata(&link).ok()?;
        if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
            return None;
        }
        std::fs::canonicalize(link).ok()
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        None
    }
}

fn validate_real_directory(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
) -> std::io::Result<()> {
    if is_link_or_reparse_point(metadata) || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("`{}` must be a real directory", path.display()),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod root_link_tests {
    use super::trusted_root_link_target;
    use std::ffi::OsStr;

    #[test]
    fn only_root_owned_links_in_the_filesystem_root_are_trusted() {
        // Real directories and missing names are not links.
        assert_eq!(trusted_root_link_target(OsStr::new("dev")), None);
        assert_eq!(
            trusted_root_link_target(OsStr::new("nia-no-such-root-entry")),
            None
        );
        // macOS reaches its temp directory through `/var -> private/var`.
        #[cfg(target_os = "macos")]
        assert_eq!(
            trusted_root_link_target(OsStr::new("var")),
            Some(std::path::PathBuf::from("/private/var"))
        );
    }
}

/// Flushes a directory entry where the host exposes directory fsync.
/// Windows does not permit opening directories through the standard File API;
/// file contents and `MoveFileExW(...WRITE_THROUGH)` still provide the
/// publication durability available on that platform.
pub fn sync_directory(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Synchronizes an existing file when the host permits flushing a read-only
/// handle. Windows requires write access for `FlushFileBuffers`; validation
/// callers only need to observe a complete file there because writers already
/// flush their writable handles before publication.
pub fn sync_file(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Release-line compatibility shared by every persisted compiler product and
/// ABI boundary. The active development line has the compatibility of the
/// release it is preparing (`0.3.0-dev` therefore uses epoch 3); a formal
/// release keeps that epoch for its patch line.
pub const PROTOCOL_EPOCH: u32 = release_compatibility(COMPILER_VERSION);
/// Stable directory component for persistent namespaces.
///
/// Each payload must match the current protocol epoch. Keeping
/// the directory spelling stable avoids another manually bumped version track;
/// incompatible entries are rejected by their owning decoder and naturally
/// replaced under the same namespace.
pub const PROTOCOL_NAMESPACE_PATH: &str = "protocol";

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

/// Current identities for generated ABI boundaries.
pub mod abi {
    /// Symbol mangling protocol epoch.
    pub const MANGLE: u64 = super::PROTOCOL_EPOCH as u64;
    /// LLVM code-generation protocol epoch.
    pub const LLVM_CODEGEN: u64 = super::PROTOCOL_EPOCH as u64;
}

/// Versions coupling the compiler to installed resources and build runners.
pub mod toolchain {
    /// Protocol epoch shared with compiled build runners.
    pub const BUILD_PROTOCOL: u32 = super::PROTOCOL_EPOCH;
}

/// Binary payload identity consisting of a diagnostic name, magic, and the
/// protocol epoch. All payloads share the same release-derived compatibility
/// value; the magic only identifies the payload kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedFormat {
    /// Human-readable registry identity.
    pub name: &'static str,
    /// Exact eight-byte prefix distinguishing this payload kind and generation.
    pub magic: &'static [u8; 8],
    /// Protocol epoch.
    pub protocol_epoch: u32,
}

impl PersistedFormat {
    /// Defines one persisted payload contract.
    pub const fn new(name: &'static str, magic: &'static [u8; 8]) -> Self {
        Self {
            name,
            magic,
            protocol_epoch: PROTOCOL_EPOCH,
        }
    }
}

/// Directory namespace for current compiler products.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedNamespace {
    /// Human-readable registry identity.
    pub name: &'static str,
    /// Protocol epoch.
    pub protocol_epoch: u32,
    /// Directory component for the current protocol namespace.
    pub path_component: &'static str,
}

impl PersistedNamespace {
    /// Defines one versioned persistence namespace.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            protocol_epoch: PROTOCOL_EPOCH,
            path_component: PROTOCOL_NAMESPACE_PATH,
        }
    }
}

/// Registered identity domain for canonical cache and action fingerprints.
///
/// Identity domains are separate from persisted payload formats: a domain
/// names the semantic inputs being fingerprinted, while a format names the
/// bytes stored on disk. Keeping the domain names here prevents independent
/// crates from silently inventing incompatible identity namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdentityDomain {
    /// Stable domain name passed to the fingerprint builder.
    pub name: &'static str,
    /// Protocol epoch that owns this identity contract.
    pub protocol_epoch: u32,
}

impl IdentityDomain {
    /// Defines one registered identity domain.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            protocol_epoch: PROTOCOL_EPOCH,
        }
    }
}

/// Authoritative registry of cross-crate build identity domains.
pub mod identities {
    use super::{IdentityDomain, PROTOCOL_EPOCH};

    /// Build invocation policy identity used by configure caching.
    pub const BUILD_INVOCATION: IdentityDomain = IdentityDomain::new("nia.build.invocation");
    /// Generated-file action key identity.
    pub const GENERATED_FILE_ACTION: IdentityDomain =
        IdentityDomain::new("nia.build.generated-file-key");
    /// Compiler-check action key identity.
    pub const COMPILER_CHECK_ACTION: IdentityDomain =
        IdentityDomain::new("nia.build.compiler-check.key");
    /// External-command action key identity.
    pub const EXTERNAL_COMMAND_ACTION: IdentityDomain =
        IdentityDomain::new("nia.build.external-command-key");
    /// Executable link-result cache key identity.
    pub const LINK_RESULT_CACHE_KEY: IdentityDomain =
        IdentityDomain::new("nia.link-result-cache-key");
    /// Static archive cache key identity.
    pub const ARCHIVE_RESULT_CACHE_KEY: IdentityDomain =
        IdentityDomain::new("nia.archive-result-cache-key");

    /// Every registered cross-crate identity domain.
    pub const ALL: &[IdentityDomain] = &[
        BUILD_INVOCATION,
        GENERATED_FILE_ACTION,
        COMPILER_CHECK_ACTION,
        EXTERNAL_COMMAND_ACTION,
        LINK_RESULT_CACHE_KEY,
        ARCHIVE_RESULT_CACHE_KEY,
    ];

    /// Keeps the registry tied to the active compatibility epoch in const
    /// contexts that import only this module.
    pub const PROTOCOL: u32 = PROTOCOL_EPOCH;
}

/// Authoritative registry of persisted payloads and cache namespaces.
pub mod formats {
    use super::{PersistedFormat, PersistedNamespace};

    /// Immutable action graph handed from a build runner to the coordinator.
    pub const BUILD_PLAN: PersistedFormat = PersistedFormat::new("build-plan", b"NIA-PLN\x01");
    /// Invocation-private configuration consumed by a compiled build runner.
    pub const RUNNER_CONFIG: PersistedFormat = PersistedFormat::new("runner-config", b"NIARUNCF");
    /// Bounded observations emitted by a host build runner.
    pub const RUNNER_OBSERVATIONS: PersistedFormat =
        PersistedFormat::new("runner-observations", b"NIAOBS\0\0");
    /// Disposable generated host-runner executable cache record.
    pub const RUNNER_EXECUTABLE_CACHE: PersistedFormat =
        PersistedFormat::new("runner-executable-cache", b"NIARUN\0\0");
    /// Versioned configure observation manifest used by the plan cache.
    pub const CONFIGURE_MANIFEST: PersistedFormat =
        PersistedFormat::new("configure-manifest", b"NIACFG\0\0");
    /// Frozen configure plan paired with its observation manifest.
    pub const CONFIGURE_RECORD: PersistedFormat =
        PersistedFormat::new("configure-record", b"NIACREC\0");

    /// Shared namespace for persistent frontend products.
    pub const FRONTEND_CACHE: PersistedNamespace = PersistedNamespace::new("frontend-cache");
    /// Private namespace for generated host-runner executables.
    pub const RUNNER_CACHE: PersistedNamespace = PersistedNamespace::new("runner-cache");
    /// Source dependency manifest for validating a frontend cache entry.
    pub const FRONTEND_DEPENDENCY_MANIFEST: PersistedFormat =
        PersistedFormat::new("frontend-dependency-manifest", b"NIAFDM\0\0");
    /// Persisted facade and reexport facts for one module.
    pub const FRONTEND_FACADE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-facade-facts", b"NIAFFF\0\x03");
    /// Persisted module dependency edges.
    pub const FRONTEND_MODULE_DEPENDENCIES: PersistedFormat =
        PersistedFormat::new("frontend-module-dependencies", b"NIAFMD\0\x03");
    /// Persisted provider candidate summary.
    pub const FRONTEND_PROVIDER_SUMMARY: PersistedFormat =
        PersistedFormat::new("frontend-provider-summary", b"NIAFPS\0\x05");
    /// Persisted fixed-point provider demand plan.
    pub const FRONTEND_PROVIDER_DEMAND_PLAN: PersistedFormat =
        PersistedFormat::new("frontend-provider-demand-plan", b"NIAFPD\0\x04");
    /// Persisted module public-surface facts.
    pub const FRONTEND_PUBLIC_SURFACE_FACTS: PersistedFormat =
        PersistedFormat::new("frontend-public-surface-facts", b"NIAFPF\0\0");

    /// Persisted type-name resolution for signatures.
    pub const SIGNATURE_TYPE_RESOLUTION: PersistedFormat =
        PersistedFormat::new("signature-type-resolution", b"NIASR\0\0\0");
    /// Persisted lowered signature types.
    pub const SIGNATURE_TYPE_LOWERING: PersistedFormat =
        PersistedFormat::new("signature-type-lowering", b"NIASL\0\0\0");
    /// Persisted item signature collection.
    pub const SIGNATURE_ITEM_SIGNATURES: PersistedFormat =
        PersistedFormat::new("signature-item-signatures", b"NIASI2\0\0");
    /// Persisted extension-validation diagnostic set.
    pub const EXTENSION_VALIDATION_DIAGNOSTICS: PersistedFormat =
        PersistedFormat::new("extension-validation-diagnostics", b"NIAEV\0\0\0");
    /// Persisted executable value-reference reachability edges.
    pub const EXECUTABLE_VALUE_REF_EDGES: PersistedFormat =
        PersistedFormat::new("executable-value-ref-edges", b"NIAER\0\0\0");
    /// Certificate proving that cached frontend checks covered their inputs.
    pub const CHECK_CERTIFICATE: PersistedFormat =
        PersistedFormat::new("check-certificate", b"NIACC\0\0\x02");
    /// Path-independent stable diagnostic bundle for one module.
    pub const STABLE_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-diagnostic-bundle", b"NIADB5\0\x01");
    /// Path-independent stable diagnostic bundle for a program.
    pub const STABLE_PROGRAM_DIAGNOSTIC_BUNDLE: PersistedFormat =
        PersistedFormat::new("stable-program-diagnostic-bundle", b"NIAPD\0\0\0");

    /// Build-action cache namespace for generated files.
    pub const GENERATED_FILE_CACHE: PersistedNamespace =
        PersistedNamespace::new("generated-file-cache");
    /// Cached generated-file action result.
    pub const GENERATED_FILE_ENTRY: PersistedFormat =
        PersistedFormat::new("generated-file-entry", b"NIAGEN\0\0");
    /// Build-action cache namespace for external commands.
    pub const EXTERNAL_COMMAND_CACHE: PersistedNamespace =
        PersistedNamespace::new("external-command-cache");
    /// Cached external-command action result.
    pub const EXTERNAL_COMMAND_ENTRY: PersistedFormat =
        PersistedFormat::new("external-command-entry", b"NIACMD\0\0");
    /// Build-action cache namespace for successful test suites.
    pub const TEST_SUITE_CACHE: PersistedNamespace = PersistedNamespace::new("test-suite-cache");
    /// Successful test-suite completion record.
    pub const TEST_SUITE_ENTRY: PersistedFormat =
        PersistedFormat::new("test-suite-entry", b"NIATST\0\0");
    /// Build-action cache namespace for compiler checks.
    pub const COMPILER_CHECK_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-check-cache");
    /// Cached compiler-check action result.
    pub const COMPILER_CHECK_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-check-entry", b"NIACKC\0\x01");
    /// Build-action cache namespace for compiler emission.
    pub const COMPILER_EMIT_CACHE: PersistedNamespace =
        PersistedNamespace::new("compiler-emit-cache");
    /// Cached compiler-emission action result.
    pub const COMPILER_EMIT_ENTRY: PersistedFormat =
        PersistedFormat::new("compiler-emit-entry", b"NIAKCE\0\x01");

    /// Namespace for recoverable output publication transactions.
    pub const OUTPUT_TRANSACTION: PersistedNamespace =
        PersistedNamespace::new("output-transaction");
    /// Journal recording output transaction state and recovery intent.
    pub const OUTPUT_TRANSACTION_JOURNAL: PersistedFormat =
        PersistedFormat::new("output-transaction-journal", b"NIATXN\0\0");
    /// Metadata for one prepared output awaiting publication.
    pub const OUTPUT_TRANSACTION_PREPARED: PersistedFormat =
        PersistedFormat::new("output-transaction-prepared", b"NIAPRP\0\0");

    /// Driver namespace for incremental object work products.
    pub const OBJECT_WORK_PRODUCT_CACHE: PersistedNamespace =
        PersistedNamespace::new("object-work-product-cache");
    /// Persisted object work product and validation metadata.
    pub const OBJECT_WORK_PRODUCT: PersistedFormat =
        PersistedFormat::new("object-work-product", b"NIAOBJ\0\0");
    /// Driver namespace for LTO pre-link bitcode work products.
    pub const LTO_MODULE_CACHE: PersistedNamespace = PersistedNamespace::new("lto-module-cache");
    /// Persisted ThinLTO pre-link bitcode and validation metadata.
    pub const THIN_LTO_MODULE: PersistedFormat =
        PersistedFormat::new("thin-lto-module", b"NIATBC\0\0");
    /// Persisted regular-LTO pre-link bitcode and validation metadata.
    pub const FULL_LTO_MODULE: PersistedFormat =
        PersistedFormat::new("full-lto-module", b"NIAFBC\0\0");
    /// Driver namespace for LLVM ThinLTO backend objects.
    pub const THIN_LTO_BACKEND_CACHE: PersistedNamespace =
        PersistedNamespace::new("thin-lto-backend-cache");
    /// Checksummed native object emitted by one LLVM ThinLTO backend task.
    pub const THIN_LTO_BACKEND_OBJECT: PersistedFormat =
        PersistedFormat::new("thin-lto-backend-object", b"NIATLTO\0");
    /// Driver namespace for executable link results.
    pub const LINK_RESULT_CACHE: PersistedNamespace = PersistedNamespace::new("link-result-cache");
    /// Persisted executable link result and component fingerprints.
    pub const LINK_RESULT: PersistedFormat = PersistedFormat::new("link-result", b"NIALNK\0\0");
    /// Driver namespace for static archive results.
    pub const ARCHIVE_RESULT_CACHE: PersistedNamespace =
        PersistedNamespace::new("archive-result-cache");
    /// Persisted static archive result and component fingerprints.
    pub const ARCHIVE_RESULT: PersistedFormat =
        PersistedFormat::new("archive-result", b"NIAARC\0\0");
    /// Complete payload registry used for uniqueness checks and audits.
    pub const ALL: &[PersistedFormat] = &[
        BUILD_PLAN,
        RUNNER_CONFIG,
        RUNNER_OBSERVATIONS,
        RUNNER_EXECUTABLE_CACHE,
        CONFIGURE_MANIFEST,
        CONFIGURE_RECORD,
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
        TEST_SUITE_ENTRY,
        COMPILER_CHECK_ENTRY,
        COMPILER_EMIT_ENTRY,
        OUTPUT_TRANSACTION_JOURNAL,
        OUTPUT_TRANSACTION_PREPARED,
        OBJECT_WORK_PRODUCT,
        THIN_LTO_MODULE,
        FULL_LTO_MODULE,
        THIN_LTO_BACKEND_OBJECT,
        LINK_RESULT,
        ARCHIVE_RESULT,
    ];

    /// Complete cache namespace registry used for version-path checks and audits.
    pub const NAMESPACES: &[PersistedNamespace] = &[
        FRONTEND_CACHE,
        RUNNER_CACHE,
        GENERATED_FILE_CACHE,
        TEST_SUITE_CACHE,
        EXTERNAL_COMMAND_CACHE,
        COMPILER_CHECK_CACHE,
        COMPILER_EMIT_CACHE,
        OUTPUT_TRANSACTION,
        OBJECT_WORK_PRODUCT_CACHE,
        LTO_MODULE_CACHE,
        THIN_LTO_BACKEND_CACHE,
        LINK_RESULT_CACHE,
        ARCHIVE_RESULT_CACHE,
    ];
}

/// Renders the canonical installed-resource manifest for this compiler build.
pub fn toolchain_manifest() -> String {
    let mut manifest = String::new();
    manifest.push_str("# Nia toolchain resource protocol identity.\n");
    manifest.push_str("compiler-version=");
    manifest.push_str(COMPILER_VERSION);
    manifest.push('\n');
    manifest.push_str("protocol-epoch=");
    manifest.push_str(&PROTOCOL_EPOCH.to_string());
    manifest.push('\n');
    manifest
}

/// Names of the source packages shipped under the resource root, each in a
/// directory of its own name.
pub const TOOLCHAIN_PACKAGES: [&str; 2] = ["std", "runtime"];

/// Renders the `nia.toml` of a toolchain source package for this compiler
/// build. Its version is the compiler version, so a release bumps one number.
pub fn toolchain_package_manifest(name: &str) -> String {
    format!(
        "[package]\nnamespace = \"nia\"\nname = \"{name}\"\nversion = \"{COMPILER_VERSION}\"\n\n[library]\npath = \"root.nia\"\n"
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{COMPILER_VERSION, PROTOCOL_EPOCH, formats, toolchain_manifest};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn protocol_epoch_tracks_the_active_release_line() {
        assert_eq!(super::release_compatibility("0.1.0"), 1);
        assert_eq!(super::release_compatibility("0.3.0-dev"), 3);
        assert_eq!(super::release_compatibility("0.3.0-dev.1"), 3);
        assert_eq!(super::release_compatibility("0.3.0"), 3);
        assert_eq!(super::release_compatibility("0.3.1"), 3);
        assert_eq!(
            super::release_compatibility(COMPILER_VERSION),
            PROTOCOL_EPOCH
        );
        assert_eq!(PROTOCOL_EPOCH, 3);
    }

    #[test]
    fn replace_path_replaces_an_existing_destination() {
        let root = std::env::temp_dir().join(format!(
            "nia-compat-replace-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create temporary root");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").expect("write source");
        fs::write(&destination, b"old").expect("write destination");
        super::replace_path(&source, &destination).expect("replace destination");
        assert_eq!(fs::read(&destination).expect("read destination"), b"new");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn persisted_identity_names_and_magics_are_unique() {
        let mut names = BTreeSet::new();
        let mut magics = BTreeSet::new();
        for format in formats::ALL {
            assert_eq!(format.protocol_epoch, PROTOCOL_EPOCH);
            assert!(
                format.protocol_epoch > 0,
                "{} has no protocol epoch",
                format.name
            );
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

        let mut identity_names = BTreeSet::new();
        for domain in super::identities::ALL {
            assert_eq!(domain.protocol_epoch, PROTOCOL_EPOCH);
            assert!(identity_names.insert(domain.name));
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
            assert_eq!(namespace.protocol_epoch, PROTOCOL_EPOCH);
            assert_eq!(namespace.path_component, super::PROTOCOL_NAMESPACE_PATH);
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

    #[test]
    fn checked_in_toolchain_package_manifests_carry_the_compiler_version() {
        let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lib");
        for name in super::TOOLCHAIN_PACKAGES {
            let path = resources.join(name).join("nia.toml");
            let checked_in = fs::read_to_string(&path).expect("read toolchain package manifest");
            assert_eq!(
                checked_in,
                super::toolchain_package_manifest(name),
                "{} must match the compiler version",
                path.display()
            );
        }
    }
}
