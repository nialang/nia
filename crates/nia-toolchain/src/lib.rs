// SPDX-License-Identifier: GPL-3.0-or-later
//! Toolchain resource discovery and compatibility identity.
//!
//! A layout binds one compiler executable to a canonical resource root, a
//! versioned manifest, the standard library, and runtime startup modules.
//! Installed toolchains use a portable prefix: `bin/nia`, `lib/`, and
//! `libexec/ld.lld` are siblings under the installation root.
//! Compatibility identity deliberately excludes filesystem paths so an intact
//! installation can be relocated without invalidating compiler caches.

use nia_compat::{COMPILER_VERSION, RELEASE_COMPATIBILITY};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};
use nia_target_config::{BuildProfile, CompilationMode, TargetConfig};
use std::{fmt, fs, io, io::Read, path::PathBuf};

const COMPATIBILITY_IDENTITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.toolchain.compatibility-identity");
const PACKAGE_TARGET_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.toolchain.package-target-path");
const MAX_RESOURCE_MANIFEST_BYTES: usize = 64 * 1024;
/// Stable package identity owned by the compiler-provided startup runtime.
pub const RUNTIME_PACKAGE_IDENTITY: &str = "toolchain:/runtime/pkg.nia";

/// Canonical symbol namespace used by the compiler-provided startup runtime.
///
/// This is derived from the same release identity used to construct
/// [`SourceRuntimeSpec::package`], so backend ownership checks cannot drift
/// when the compiler release changes.
pub fn runtime_symbol_package_identity() -> String {
    nia_package_metadata::PackageId {
        namespace: "nia".to_string(),
        name: "runtime".to_string(),
        version: format!("{}+runtime{}", COMPILER_VERSION, RELEASE_COMPATIBILITY),
    }
    .canonical_text()
}

/// File name of the versioned compatibility manifest under a resource root.
pub const RESOURCE_MANIFEST_NAME: &str = "toolchain.meta";

/// Path-independent compatibility identity read from `toolchain.meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainIdentity {
    compiler_version: String,
    release_compatibility: u32,
}

/// Stable fingerprint of all fields in [`ToolchainIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolchainIdentityFingerprint(nia_query::QueryFingerprint);

impl ToolchainIdentityFingerprint {
    /// Returns the identity fingerprint expected by this compiler build.
    pub fn current() -> Self {
        ToolchainIdentity {
            compiler_version: COMPILER_VERSION.to_string(),
            release_compatibility: RELEASE_COMPATIBILITY,
        }
        .fingerprint()
    }

    /// Reconstructs a persisted fingerprint from its two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(nia_query::QueryFingerprint::from_parts(parts))
    }

    /// Returns the two 64-bit lanes for persistence.
    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}

impl ToolchainIdentity {
    /// Returns the compiler version required by the resource bundle.
    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }

    /// Returns the release-scoped compatibility epoch shared by all
    /// toolchain resources, persisted artifacts, and ABI boundaries.
    pub const fn release_compatibility(&self) -> u32 {
        self.release_compatibility
    }

    /// Computes a deterministic fingerprint over every compatibility field.
    pub fn fingerprint(&self) -> ToolchainIdentityFingerprint {
        let mut builder = QueryFingerprintBuilder::new(COMPATIBILITY_IDENTITY_DOMAIN);
        builder.write_str(&self.compiler_version);
        builder.write_u64(u64::from(self.release_compatibility));
        ToolchainIdentityFingerprint(builder.finish())
    }
}

/// Runtime dependency made visible to a private runtime source package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeDependency {
    /// The package containing the user-selected entry module.
    EntryPackage,
    /// The standard-library package selected for this compilation.
    StandardLibrary,
}

/// External entry point selected from a runtime source package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEntryPoint {
    module_identity: String,
    definition_name: String,
    linker_symbol: String,
}

impl RuntimeEntryPoint {
    /// Returns the exact logical module containing the external entry.
    pub fn module_identity(&self) -> &str {
        &self.module_identity
    }

    /// Returns the exact source-level definition rooted by the compiler.
    pub fn definition_name(&self) -> &str {
        &self.definition_name
    }

    /// Returns the externally visible linker symbol rooted by the driver.
    pub fn linker_symbol(&self) -> &str {
        &self.linker_symbol
    }
}

/// Validated private source package implementing executable startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRuntimeSpec {
    package_root: PathBuf,
    package_root_identity: String,
    package: nia_package_metadata::PackageId,
    entry_point: RuntimeEntryPoint,
    target: TargetConfig,
    dependencies: Vec<RuntimeDependency>,
}

impl SourceRuntimeSpec {
    /// Validated physical root of the private runtime source package.
    pub fn package_root(&self) -> &std::path::Path {
        &self.package_root
    }

    /// Stable identity of the private runtime package facade.
    pub fn package_root_identity(&self) -> &str {
        &self.package_root_identity
    }

    /// Stable package identity owning source and cached native startup units.
    pub fn package(&self) -> &nia_package_metadata::PackageId {
        &self.package
    }

    /// Exact external entry point supplied by this runtime.
    pub fn entry_point(&self) -> &RuntimeEntryPoint {
        &self.entry_point
    }

    /// Target configuration for which this runtime was selected.
    pub fn target(&self) -> &TargetConfig {
        &self.target
    }

    /// Explicit package bindings visible to runtime source.
    pub fn dependencies(&self) -> &[RuntimeDependency] {
        &self.dependencies
    }
}

/// Canonical startup selection shared by driver, loader, and compiler.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RuntimeSpec {
    /// Compile without injecting executable startup resources.
    #[default]
    Bare,
    /// Inject and root a validated private runtime source package.
    Source(SourceRuntimeSpec),
}

impl RuntimeSpec {
    /// Returns the selected source runtime, if executable startup is enabled.
    pub fn source(&self) -> Option<&SourceRuntimeSpec> {
        match self {
            Self::Bare => None,
            Self::Source(runtime) => Some(runtime),
        }
    }

    /// Builds the freestanding runtime shipped by `toolchain` for `target`.
    pub fn freestanding(
        toolchain: &ToolchainLayout,
        target: &TargetConfig,
    ) -> Result<Self, RuntimeSpecError> {
        Self::freestanding_from_package_root(toolchain.freestanding_runtime_package(), target)
    }

    /// Builds a freestanding runtime from an already validated start module.
    ///
    /// Toolchain consumers should normally use [`Self::freestanding`]. This
    /// constructor lets in-memory compiler fixtures preserve the same runtime
    /// identity and target validation without constructing a second mode flag.
    pub fn freestanding_from_package_root(
        package_root: impl Into<PathBuf>,
        target: &TargetConfig,
    ) -> Result<Self, RuntimeSpecError> {
        let implementation = match (target.os.as_str(), target.arch.as_str()) {
            ("linux", "x86_64") => "x86_64",
            ("linux", "x86" | "i386" | "i586" | "i686") => "x86",
            _ => {
                return Err(RuntimeSpecError::UnsupportedTarget {
                    arch: target.arch.clone(),
                    os: target.os.clone(),
                });
            }
        };
        Ok(Self::Source(SourceRuntimeSpec {
            package_root: package_root.into(),
            package_root_identity: RUNTIME_PACKAGE_IDENTITY.to_string(),
            package: nia_package_metadata::PackageId {
                namespace: "nia".to_string(),
                name: "runtime".to_string(),
                version: format!("{}+runtime{}", COMPILER_VERSION, RELEASE_COMPATIBILITY),
            },
            entry_point: RuntimeEntryPoint {
                module_identity: format!(
                    "toolchain:/runtime/start/freestanding/linux/{implementation}.nia"
                ),
                definition_name: "_start".to_string(),
                linker_symbol: "_start".to_string(),
            },
            target: target.clone(),
            dependencies: vec![
                RuntimeDependency::EntryPackage,
                RuntimeDependency::StandardLibrary,
            ],
        }))
    }

    /// Validates that this runtime can participate in a request for `target`.
    pub fn validate_for_target(&self, target: &TargetConfig) -> Result<(), RuntimeSpecError> {
        let Self::Source(runtime) = self else {
            return Ok(());
        };
        if runtime.target != *target {
            return Err(RuntimeSpecError::TargetMismatch {
                runtime: Box::new(runtime.target.clone()),
                request: Box::new(target.clone()),
            });
        }
        if runtime.dependencies.as_slice()
            != [
                RuntimeDependency::EntryPackage,
                RuntimeDependency::StandardLibrary,
            ]
        {
            return Err(RuntimeSpecError::InvalidDependencies);
        }
        Ok(())
    }
}

/// Failure to select a runtime implementation for a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSpecError {
    /// The installed runtime has no implementation for this target pair.
    UnsupportedTarget { arch: String, os: String },
    /// A validated runtime was reused for a different compilation target.
    TargetMismatch {
        runtime: Box<TargetConfig>,
        request: Box<TargetConfig>,
    },
    /// The runtime package dependency contract is not supported.
    InvalidDependencies,
}

impl fmt::Display for RuntimeSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTarget { arch, os } => {
                write!(f, "no freestanding runtime for target `{arch}-{os}`")
            }
            Self::TargetMismatch { runtime, request } => write!(
                f,
                "runtime target `{}-{}` does not match request target `{}-{}`",
                runtime.arch, runtime.os, request.arch, request.os
            ),
            Self::InvalidDependencies => {
                write!(
                    f,
                    "runtime dependencies must contain exactly the entry package"
                )
            }
        }
    }
}

impl std::error::Error for RuntimeSpecError {}

/// Runtime source modules shipped in the resolved resource bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResources {
    freestanding_package_root: PathBuf,
}

impl RuntimeResources {
    /// Returns the private source package implementing freestanding startup.
    pub fn freestanding_package_root(&self) -> &std::path::Path {
        &self.freestanding_package_root
    }
}

/// Validated compiler executable, resources, identity, and target configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainLayout {
    compiler_executable: PathBuf,
    resource_root: PathBuf,
    std_module: PathBuf,
    identity: ToolchainIdentity,
    host_target: TargetConfig,
    artifact_target: TargetConfig,
    runtime: RuntimeResources,
    bundled_lld: Option<PathBuf>,
}

impl ToolchainLayout {
    /// Resolves and validates every resource named by `request`.
    ///
    /// The resource root is canonicalized before exposure. The manifest must
    /// exactly match this compiler's release compatibility and compiler
    /// version; required standard-library and runtime files must
    /// be regular files.
    pub fn resolve(request: ToolchainLayoutRequest) -> Result<Self, ToolchainLayoutError> {
        validate_file(
            &request.compiler_executable,
            ResourceRole::CompilerExecutable,
        )?;
        let root = match request.resources {
            ResourceRootSelection::Installed => request
                .compiler_executable
                .parent()
                .ok_or_else(|| ToolchainLayoutError::MissingExecutableParent {
                    path: request.compiler_executable.clone(),
                })?
                .join("../lib"),
            ResourceRootSelection::Explicit(root) => root,
        };
        let resource_root =
            fs::canonicalize(&root).map_err(|error| ToolchainLayoutError::ReadResourceRoot {
                path: root.clone(),
                error,
            })?;
        if !resource_root.is_dir() {
            return Err(ToolchainLayoutError::NotDirectory {
                role: ResourceRole::ResourceRoot,
                path: resource_root,
            });
        }

        let manifest_path = resource_root.join(RESOURCE_MANIFEST_NAME);
        let manifest = read_resource_manifest(&manifest_path).map_err(|error| {
            ToolchainLayoutError::ReadManifest {
                path: manifest_path.clone(),
                error,
            }
        })?;
        let identity = parse_manifest(&manifest_path, &manifest)?;
        validate_identity(&manifest_path, &identity)?;

        let std_module = resource_root.join("std/pkg.nia");
        validate_file(&std_module, ResourceRole::StandardLibrary)?;
        let freestanding_package_root = resource_root.join("runtime/pkg.nia");
        validate_file(
            &freestanding_package_root,
            ResourceRole::FreestandingRuntime,
        )?;
        let bundled_lld = resource_root
            .parent()
            .map(|prefix| prefix.join("libexec/ld.lld"));
        let bundled_lld = match bundled_lld {
            Some(path) if path.exists() => {
                Some(validate_optional_file(&path, ResourceRole::BundledLinker)?)
            }
            _ => None,
        };

        Ok(Self {
            compiler_executable: request.compiler_executable,
            resource_root,
            std_module,
            identity,
            host_target: TargetConfig::host(),
            artifact_target: request.artifact_target,
            runtime: RuntimeResources {
                freestanding_package_root,
            },
            bundled_lld,
        })
    }

    /// Returns the compiler executable supplied by the request.
    pub fn compiler_executable(&self) -> &std::path::Path {
        &self.compiler_executable
    }

    /// Returns the canonical resource root.
    pub fn resource_root(&self) -> &std::path::Path {
        &self.resource_root
    }

    /// Returns the validated standard-library root module.
    pub fn std_module(&self) -> &std::path::Path {
        &self.std_module
    }

    /// Returns the validated private source package for freestanding startup.
    ///
    /// Runtime startup is a toolchain resource, not a module owned by the
    /// standard-library package artifact.
    pub fn freestanding_runtime_package(&self) -> &std::path::Path {
        self.runtime.freestanding_package_root()
    }

    /// Compiled-package snapshot for one standard-library semantic context.
    ///
    /// The artifact is optional during development and diagnostics; callers
    /// must still validate its manifest before selecting it.
    pub fn std_package_artifact(
        &self,
        target: &TargetConfig,
        profile: BuildProfile,
        compilation_mode: CompilationMode,
    ) -> std::path::PathBuf {
        let mut fingerprint = QueryFingerprintBuilder::new(PACKAGE_TARGET_PATH_DOMAIN);
        fingerprint.write_str(&target.arch);
        fingerprint.write_str(&target.vendor);
        fingerprint.write_str(&target.os);
        fingerprint.write_str(&target.env);
        fingerprint.write_str(&target.abi);
        fingerprint.write_str(&target.endian);
        fingerprint.write_u64(u64::from(target.pointer_width));
        let [first, second] = fingerprint.finish().parts();
        let profile = match profile {
            BuildProfile::Debug => "debug",
            BuildProfile::Release => "release",
        };
        let compilation_mode = match compilation_mode {
            CompilationMode::Normal => "normal",
            CompilationMode::Test => "test",
        };
        self.resource_root
            .join("std/.nia-cache/packages")
            .join(format!("{first:016x}{second:016x}"))
            .join(profile)
            .join(compilation_mode)
            .join("package.niapkg")
    }

    /// Returns the canonical package identity expected from the standard
    /// library artifact shipped with this toolchain.
    pub fn std_package_id(&self) -> nia_package_metadata::PackageId {
        nia_package_metadata::PackageId::standard_library()
    }

    /// Returns the manifest compatibility identity.
    pub const fn identity(&self) -> &ToolchainIdentity {
        &self.identity
    }

    /// Returns the host target used to execute compiler-side tools.
    pub const fn host_target(&self) -> &TargetConfig {
        &self.host_target
    }

    /// Returns the independently selected target for produced artifacts.
    pub const fn artifact_target(&self) -> &TargetConfig {
        &self.artifact_target
    }

    /// Returns validated runtime resource paths.
    pub const fn runtime(&self) -> &RuntimeResources {
        &self.runtime
    }

    /// Returns the bundled LLD executable when the resource tree provides one.
    pub fn bundled_lld(&self) -> Option<&std::path::Path> {
        self.bundled_lld.as_deref()
    }
}

fn read_resource_manifest(path: &std::path::Path) -> io::Result<String> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_RESOURCE_MANIFEST_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "toolchain resource manifest exceeds the {MAX_RESOURCE_MANIFEST_BYTES}-byte limit"
            ),
        ));
    }
    let mut encoded = Vec::new();
    file.take((MAX_RESOURCE_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_RESOURCE_MANIFEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "toolchain resource manifest exceeds the {MAX_RESOURCE_MANIFEST_BYTES}-byte limit"
            ),
        ));
    }
    String::from_utf8(encoded).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Inputs selecting a compiler executable, resource root, and artifact target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainLayoutRequest {
    compiler_executable: PathBuf,
    resources: ResourceRootSelection,
    artifact_target: TargetConfig,
}

impl ToolchainLayoutRequest {
    /// Selects resources at `../lib` relative to the executable directory.
    pub fn installed(compiler_executable: impl Into<PathBuf>) -> Self {
        Self {
            compiler_executable: compiler_executable.into(),
            resources: ResourceRootSelection::Installed,
            artifact_target: TargetConfig::host(),
        }
    }

    /// Selects an explicit resource root, primarily for development layouts.
    pub fn explicit(
        compiler_executable: impl Into<PathBuf>,
        resource_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            compiler_executable: compiler_executable.into(),
            resources: ResourceRootSelection::Explicit(resource_root.into()),
            artifact_target: TargetConfig::host(),
        }
    }

    /// Overrides the artifact target while leaving compiler tools on the host target.
    pub fn with_artifact_target(mut self, target: TargetConfig) -> Self {
        self.artifact_target = target;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResourceRootSelection {
    Installed,
    Explicit(PathBuf),
}

/// Semantic role of a required toolchain filesystem resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceRole {
    /// Compiler executable used to invoke this toolchain.
    CompilerExecutable,
    /// Directory containing the manifest and shipped sources.
    ResourceRoot,
    /// Root source module of the standard library.
    StandardLibrary,
    /// Startup source module for freestanding executables.
    FreestandingRuntime,
    /// LLVM LLD executable shipped with a release resource tree.
    BundledLinker,
}

impl fmt::Display for ResourceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CompilerExecutable => "compiler executable",
            Self::ResourceRoot => "toolchain resource root",
            Self::StandardLibrary => "standard-library root module",
            Self::FreestandingRuntime => "freestanding runtime module",
            Self::BundledLinker => "bundled LLD linker",
        })
    }
}

/// Failure to discover, parse, or validate a toolchain layout.
#[derive(Debug)]
pub enum ToolchainLayoutError {
    /// An installed-layout request used an executable path with no parent.
    MissingExecutableParent {
        /// Requested compiler executable path.
        path: PathBuf,
    },
    /// The selected resource root could not be canonicalized.
    ReadResourceRoot {
        /// Selected resource root path.
        path: PathBuf,
        /// Filesystem failure.
        error: io::Error,
    },
    /// A resource expected to be a directory has another file type.
    NotDirectory {
        /// Resource being validated.
        role: ResourceRole,
        /// Resource path.
        path: PathBuf,
    },
    /// The compatibility manifest could not be read as UTF-8 text.
    ReadManifest {
        /// Manifest path.
        path: PathBuf,
        /// Filesystem or decoding failure.
        error: io::Error,
    },
    /// Metadata for a required resource could not be read.
    ReadResource {
        /// Resource being validated.
        role: ResourceRole,
        /// Resource path.
        path: PathBuf,
        /// Filesystem failure.
        error: io::Error,
    },
    /// A manifest line has invalid syntax, an unknown/duplicate field, or an invalid number.
    MalformedManifest {
        /// Manifest path.
        path: PathBuf,
        /// One-based source line containing the error.
        line: usize,
        /// Description of the malformed input.
        message: String,
    },
    /// A required compatibility field is absent.
    MissingManifestField {
        /// Manifest path.
        path: PathBuf,
        /// Missing field name.
        field: &'static str,
    },
    /// A compatibility field does not match this compiler build.
    IncompatibleManifestField {
        /// Manifest path.
        path: PathBuf,
        /// Incompatible field name.
        field: &'static str,
        /// Value required by this compiler.
        expected: String,
        /// Value found in the manifest.
        found: String,
    },
    /// A required resource does not exist.
    MissingResource {
        /// Missing resource role.
        role: ResourceRole,
        /// Expected resource path.
        path: PathBuf,
    },
    /// A required regular file has another file type.
    NotFile {
        /// Resource being validated.
        role: ResourceRole,
        /// Resource path.
        path: PathBuf,
    },
}

impl fmt::Display for ToolchainLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingExecutableParent { path } => write!(
                f,
                "compiler executable `{}` has no parent directory",
                path.display()
            ),
            Self::ReadResourceRoot { path, error } => write!(
                f,
                "failed to resolve toolchain resource root `{}`: {error}",
                path.display()
            ),
            Self::NotDirectory { role, path } => {
                write!(f, "{role} `{}` is not a directory", path.display())
            }
            Self::ReadManifest { path, error } => write!(
                f,
                "failed to read toolchain resource manifest `{}`: {error}",
                path.display()
            ),
            Self::ReadResource { role, path, error } => {
                write!(f, "failed to read {role} `{}`: {error}", path.display())
            }
            Self::MalformedManifest {
                path,
                line,
                message,
            } => write!(
                f,
                "malformed toolchain resource manifest `{}` at line {line}: {message}",
                path.display()
            ),
            Self::MissingManifestField { path, field } => write!(
                f,
                "toolchain resource manifest `{}` is missing `{field}`",
                path.display()
            ),
            Self::IncompatibleManifestField {
                path,
                field,
                expected,
                found,
            } => write!(
                f,
                "incompatible toolchain resource manifest `{}`: `{field}` must be `{expected}`, found `{found}`",
                path.display()
            ),
            Self::MissingResource { role, path } => {
                write!(f, "missing {role} `{}`", path.display())
            }
            Self::NotFile { role, path } => {
                write!(f, "{role} `{}` is not a file", path.display())
            }
        }
    }
}

impl std::error::Error for ToolchainLayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadResourceRoot { error, .. }
            | Self::ReadManifest { error, .. }
            | Self::ReadResource { error, .. } => Some(error),
            _ => None,
        }
    }
}

fn validate_file(path: &std::path::Path, role: ResourceRole) -> Result<(), ToolchainLayoutError> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ToolchainLayoutError::MissingResource {
                role,
                path: path.to_path_buf(),
            }
        } else {
            ToolchainLayoutError::ReadResource {
                role,
                path: path.to_path_buf(),
                error,
            }
        }
    })?;
    if !metadata.is_file() {
        return Err(ToolchainLayoutError::NotFile {
            role,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_optional_file(
    path: &std::path::Path,
    role: ResourceRole,
) -> Result<PathBuf, ToolchainLayoutError> {
    let metadata = fs::metadata(path).map_err(|error| ToolchainLayoutError::ReadResource {
        role,
        path: path.to_path_buf(),
        error,
    })?;
    if !metadata.is_file() {
        return Err(ToolchainLayoutError::NotFile {
            role,
            path: path.to_path_buf(),
        });
    }
    Ok(path.to_path_buf())
}

#[derive(Default)]
struct ManifestFields {
    compiler_version: Option<ManifestValue>,
    release_compatibility: Option<ManifestValue>,
}

struct ManifestValue {
    text: String,
    line: usize,
}

fn parse_manifest(
    path: &std::path::Path,
    text: &str,
) -> Result<ToolchainIdentity, ToolchainLayoutError> {
    let mut fields = ManifestFields::default();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(malformed(path, index, "expected `name=value`"));
        };
        let name = name.trim();
        let value = value.trim();
        if value.is_empty() {
            return Err(malformed(path, index, format!("`{name}` cannot be empty")));
        }
        let slot = match name {
            "compiler-version" => &mut fields.compiler_version,
            "release-compatibility" => &mut fields.release_compatibility,
            _ => {
                return Err(malformed(path, index, format!("unknown field `{name}`")));
            }
        };
        if slot
            .replace(ManifestValue {
                text: value.to_string(),
                line: index + 1,
            })
            .is_some()
        {
            return Err(malformed(path, index, format!("duplicate field `{name}`")));
        }
    }

    Ok(ToolchainIdentity {
        compiler_version: required(path, "compiler-version", fields.compiler_version)?.text,
        release_compatibility: parse_u32(
            path,
            "release-compatibility",
            required(path, "release-compatibility", fields.release_compatibility)?,
        )?,
    })
}

fn malformed(
    path: &std::path::Path,
    zero_based_line: usize,
    message: impl Into<String>,
) -> ToolchainLayoutError {
    ToolchainLayoutError::MalformedManifest {
        path: path.to_path_buf(),
        line: zero_based_line + 1,
        message: message.into(),
    }
}

fn required(
    path: &std::path::Path,
    field: &'static str,
    value: Option<ManifestValue>,
) -> Result<ManifestValue, ToolchainLayoutError> {
    value.ok_or_else(|| ToolchainLayoutError::MissingManifestField {
        path: path.to_path_buf(),
        field,
    })
}

fn parse_u32(
    path: &std::path::Path,
    field: &'static str,
    value: ManifestValue,
) -> Result<u32, ToolchainLayoutError> {
    value
        .text
        .parse()
        .map_err(|_| ToolchainLayoutError::MalformedManifest {
            path: path.to_path_buf(),
            line: value.line,
            message: format!(
                "`{field}` must be an unsigned 32-bit integer, found `{}`",
                value.text
            ),
        })
}

fn validate_identity(
    path: &std::path::Path,
    identity: &ToolchainIdentity,
) -> Result<(), ToolchainLayoutError> {
    validate_field(
        path,
        "compiler-version",
        COMPILER_VERSION.to_string(),
        identity.compiler_version.clone(),
    )?;
    validate_field(
        path,
        "release-compatibility",
        RELEASE_COMPATIBILITY.to_string(),
        identity.release_compatibility.to_string(),
    )
}

fn validate_field(
    path: &std::path::Path,
    field: &'static str,
    expected: String,
    found: String,
) -> Result<(), ToolchainLayoutError> {
    if expected == found {
        Ok(())
    } else {
        Err(ToolchainLayoutError::IncompatibleManifestField {
            path: path.to_path_buf(),
            field,
            expected,
            found,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("nia-toolchain-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&root).expect("create toolchain test root");
        root
    }

    fn write_layout(root: &std::path::Path) -> PathBuf {
        let executable = root.join("bin/nia");
        fs::create_dir_all(executable.parent().expect("bin parent")).expect("create bin");
        fs::write(&executable, b"compiler").expect("write compiler");
        let resources = root.join("lib");
        fs::create_dir_all(resources.join("std")).expect("create std directory");
        fs::create_dir_all(resources.join("runtime")).expect("create runtime directory");
        fs::write(
            resources.join(RESOURCE_MANIFEST_NAME),
            nia_compat::toolchain_manifest(),
        )
        .expect("write manifest");
        fs::write(resources.join("std/pkg.nia"), "pub module start;").expect("write std root");
        fs::write(resources.join("runtime/pkg.nia"), "pub(pkg) module start;")
            .expect("write runtime root");
        fs::write(resources.join("runtime/start.nia"), "").expect("write runtime");
        executable
    }

    #[test]
    fn resolves_explicit_and_installed_layouts_with_path_independent_identity() {
        let first = temp_dir("resolves_layout");
        let executable = write_layout(&first);
        let explicit = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            &executable,
            first.join("lib"),
        ))
        .expect("explicit layout");
        let installed = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("installed layout");
        assert_eq!(explicit, installed);
        let artifact = explicit.std_package_artifact(
            &TargetConfig::host(),
            BuildProfile::Debug,
            CompilationMode::Normal,
        );
        assert!(artifact.starts_with(explicit.resource_root().join("std/.nia-cache/packages")));
        assert!(artifact.ends_with("debug/normal/package.niapkg"));
        assert_ne!(
            artifact,
            explicit.std_package_artifact(
                &TargetConfig::host(),
                BuildProfile::Release,
                CompilationMode::Normal,
            )
        );
        assert_ne!(
            artifact,
            explicit.std_package_artifact(
                &TargetConfig::host(),
                BuildProfile::Debug,
                CompilationMode::Test,
            )
        );
        let mut other_target = TargetConfig::host();
        other_target.arch = "other-arch".into();
        assert_ne!(
            artifact,
            explicit.std_package_artifact(
                &other_target,
                BuildProfile::Debug,
                CompilationMode::Normal,
            )
        );

        let relocated_root = temp_dir("relocated_layout");
        fs::rename(&first, relocated_root.join("toolchain")).expect("relocate toolchain");
        let relocated = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(
            relocated_root.join("toolchain/bin/nia"),
        ))
        .expect("relocated layout");
        assert_eq!(relocated.identity(), installed.identity());
        assert_ne!(relocated.resource_root(), installed.resource_root());
        assert_eq!(
            artifact.strip_prefix(explicit.resource_root()).unwrap(),
            relocated
                .std_package_artifact(
                    &TargetConfig::host(),
                    BuildProfile::Debug,
                    CompilationMode::Normal,
                )
                .strip_prefix(relocated.resource_root())
                .unwrap()
        );
    }

    #[test]
    fn freestanding_runtime_has_stable_source_and_abi_identities() {
        let root = temp_dir("freestanding_runtime_spec");
        let executable = write_layout(&root);
        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("installed layout");
        let target = TargetConfig::host();
        let runtime = RuntimeSpec::freestanding(&layout, &target).expect("host runtime");
        let source = runtime.source().expect("source runtime");

        assert_eq!(source.package_root_identity(), RUNTIME_PACKAGE_IDENTITY);
        assert_eq!(
            source.package().canonical_text(),
            runtime_symbol_package_identity()
        );
        assert_eq!(source.package_root(), root.join("lib/runtime/pkg.nia"));
        let implementation = if target.arch == "x86_64" {
            "x86_64"
        } else {
            "x86"
        };
        assert_eq!(
            source.entry_point().module_identity(),
            format!("toolchain:/runtime/start/freestanding/linux/{implementation}.nia")
        );
        assert_eq!(source.entry_point().definition_name(), "_start");
        assert_eq!(source.entry_point().linker_symbol(), "_start");
        assert_eq!(
            source.dependencies(),
            [
                RuntimeDependency::EntryPackage,
                RuntimeDependency::StandardLibrary
            ]
        );
        assert_eq!(source.target(), &target);
        assert_eq!(runtime.validate_for_target(&target), Ok(()));
    }

    #[test]
    fn runtime_selection_rejects_unsupported_and_mismatched_targets() {
        let unsupported = TargetConfig {
            arch: "aarch64".to_string(),
            ..TargetConfig::host()
        };
        assert!(matches!(
            RuntimeSpec::freestanding_from_package_root("runtime/pkg.nia", &unsupported),
            Err(RuntimeSpecError::UnsupportedTarget { .. })
        ));

        let target = TargetConfig::host();
        let runtime =
            RuntimeSpec::freestanding_from_package_root("runtime/pkg.nia", &target).unwrap();
        let mut different = target;
        different.abi = "different".to_string();
        assert!(matches!(
            runtime.validate_for_target(&different),
            Err(RuntimeSpecError::TargetMismatch { .. })
        ));
    }

    #[test]
    fn exposes_bundled_lld_from_release_layout() {
        let root = temp_dir("bundled_lld");
        let executable = write_layout(&root);
        let lld = root.join("libexec/ld.lld");
        fs::create_dir_all(lld.parent().expect("lld parent")).expect("create lld directory");
        fs::write(&lld, b"lld").expect("write bundled lld");

        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("release layout");
        assert_eq!(layout.bundled_lld(), Some(lld.as_path()));
    }

    #[test]
    fn rejects_missing_and_incompatible_resources() {
        let root = temp_dir("rejects_resources");
        let executable = write_layout(&root);
        fs::write(
            root.join("lib/toolchain.meta"),
            "release-compatibility=1\ncompiler-version=incompatible\n",
        )
        .expect("replace manifest");
        let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect_err("incompatible compiler version");
        assert!(error.to_string().contains("`compiler-version`"), "{error}");

        fs::remove_file(root.join("lib/toolchain.meta")).expect("remove manifest");
        let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect_err("missing manifest");
        assert!(error.to_string().contains("resource manifest"), "{error}");
    }

    #[test]
    fn malformed_numeric_manifest_field_reports_its_source_line() {
        let path = PathBuf::from("toolchain.meta");
        let manifest = format!(
            "# identity\ncompiler-version={COMPILER_VERSION}\nrelease-compatibility=invalid\n"
        );
        let error = parse_manifest(&path, &manifest).expect_err("invalid numeric schema");

        assert!(
            matches!(
                error,
                ToolchainLayoutError::MalformedManifest { line: 3, .. }
            ),
            "{error}"
        );
    }

    #[test]
    fn oversized_resource_manifest_is_rejected_without_parsing_its_prefix() {
        let root = temp_dir("oversized_manifest");
        let executable = write_layout(&root);
        let manifest = root.join("lib/toolchain.meta");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&manifest)
            .expect("open manifest");
        file.set_len((MAX_RESOURCE_MANIFEST_BYTES + 1) as u64)
            .expect("extend manifest");

        let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect_err("oversized manifest");

        assert!(
            matches!(
                error,
                ToolchainLayoutError::ReadManifest { ref error, .. }
                    if error.kind() == io::ErrorKind::InvalidData
            ),
            "{error}"
        );
        assert!(error.to_string().contains("65536-byte limit"), "{error}");
    }

    #[test]
    fn compatibility_fingerprint_is_path_independent_and_tracks_every_identity_field() {
        let baseline = ToolchainIdentity {
            compiler_version: "compiler".to_string(),
            release_compatibility: 1,
        };
        let baseline_fingerprint = baseline.fingerprint();
        for changed in [
            ToolchainIdentity {
                compiler_version: "changed".to_string(),
                ..baseline.clone()
            },
            ToolchainIdentity {
                release_compatibility: 9,
                ..baseline.clone()
            },
        ] {
            assert_ne!(baseline_fingerprint, changed.fingerprint());
        }

        assert_eq!(
            ToolchainIdentityFingerprint::current(),
            ToolchainIdentity {
                compiler_version: COMPILER_VERSION.to_string(),
                release_compatibility: RELEASE_COMPATIBILITY,
            }
            .fingerprint()
        );
    }
}
