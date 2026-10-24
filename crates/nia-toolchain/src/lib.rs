// SPDX-License-Identifier: GPL-3.0-or-later
//! Toolchain resource discovery and compatibility identity.
//!
//! A layout binds one compiler executable to a canonical resource root, a
//! versioned manifest, the standard library, and runtime startup modules.
//! Installed toolchains use a portable prefix: `bin/nia`, `lib/`, and
//! `libexec/ld.lld` (with the host executable suffix) are siblings under the
//! installation root. The linker selects each target's LLD flavor explicitly,
//! so the one bundled binary links every target.
//! Compatibility identity deliberately excludes filesystem paths so an intact
//! installation can be relocated without invalidating compiler caches.

use nia_compat::{COMPILER_VERSION, PROTOCOL_EPOCH};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};
use nia_target::{Arch, Os, TargetConfig};
use std::{fmt, fs, io, io::Read, path::PathBuf};

const COMPATIBILITY_IDENTITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.toolchain.compatibility-identity");
const MAX_RESOURCE_MANIFEST_BYTES: usize = 64 * 1024;

/// File name of the versioned compatibility manifest under a resource root.
pub const RESOURCE_MANIFEST_NAME: &str = "toolchain.meta";

/// Path-independent compatibility identity read from `toolchain.meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainIdentity {
    compiler_version: String,
    protocol_epoch: u32,
}

/// Stable fingerprint of all fields in [`ToolchainIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolchainIdentityFingerprint(nia_query::QueryFingerprint);

impl ToolchainIdentityFingerprint {
    /// Returns the identity fingerprint expected by this compiler build.
    pub fn current() -> Self {
        ToolchainIdentity {
            compiler_version: COMPILER_VERSION.to_string(),
            protocol_epoch: PROTOCOL_EPOCH,
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

    /// Returns the current protocol epoch shared by all
    /// toolchain resources, persisted artifacts, and ABI boundaries.
    pub const fn protocol_epoch(&self) -> u32 {
        self.protocol_epoch
    }

    /// Computes a deterministic fingerprint over every compatibility field.
    pub fn fingerprint(&self) -> ToolchainIdentityFingerprint {
        let mut builder = QueryFingerprintBuilder::new(COMPATIBILITY_IDENTITY_DOMAIN);
        builder.write_str(&self.compiler_version);
        builder.write_u64(u64::from(self.protocol_epoch));
        ToolchainIdentityFingerprint(builder.finish())
    }
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

/// A source definition retained even when only native codegen references it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeExport {
    module_identity: String,
    definition_name: String,
}

impl RuntimeExport {
    /// Selects an exact definition in the runtime source package.
    pub fn new(module_identity: impl Into<String>, definition_name: impl Into<String>) -> Self {
        Self {
            module_identity: module_identity.into(),
            definition_name: definition_name.into(),
        }
    }

    /// Stable source identity of the module owning this definition.
    pub fn module_identity(&self) -> &str {
        &self.module_identity
    }

    /// Source-level name of the runtime-owned native export.
    pub fn definition_name(&self) -> &str {
        &self.definition_name
    }
}

/// Validated private source package implementing executable startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRuntimeSpec {
    package: ToolchainPackage,
    entry_point: RuntimeEntryPoint,
    required_exports: Vec<RuntimeExport>,
    target: TargetConfig,
}

impl SourceRuntimeSpec {
    /// The runtime source package.
    pub fn package(&self) -> &ToolchainPackage {
        &self.package
    }

    /// Exact external entry point supplied by this runtime.
    pub fn entry_point(&self) -> &RuntimeEntryPoint {
        &self.entry_point
    }

    /// Additional runtime-owned C ABI definitions required by native codegen.
    pub fn required_exports(&self) -> &[RuntimeExport] {
        &self.required_exports
    }

    /// Target configuration for which this runtime was selected.
    pub fn target(&self) -> &TargetConfig {
        &self.target
    }
}

/// Canonical startup selection shared by driver, loader, and compiler.
/// This selects Nia startup injection, independently of OS or library linkage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RuntimeSpec {
    /// Compile without injecting executable startup resources.
    #[default]
    Bare,
    /// Inject and root a validated private runtime source package.
    Source(Box<SourceRuntimeSpec>),
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
        Self::freestanding_from_package(toolchain.freestanding_runtime_package(), target)
    }

    /// Builds a freestanding runtime from an already validated package.
    ///
    /// Toolchain consumers should normally use [`Self::freestanding`]. This
    /// constructor lets in-memory compiler fixtures preserve the same runtime
    /// identity and target validation without constructing a second mode flag.
    pub fn freestanding_from_package(
        package: &ToolchainPackage,
        target: &TargetConfig,
    ) -> Result<Self, RuntimeSpecError> {
        // Native entry points call their startup helper by symbol name, so the
        // helper is rooted explicitly rather than through a code operand.
        let startup = match target.os() {
            Os::Linux => ("start/freestanding/linux/startup.nia", "niaStartStack"),
            Os::Windows => ("start/freestanding/windows/startup.nia", "niaWindowsStart"),
            Os::Macos => ("start/freestanding/macos/startup.nia", "niaDarwinStart"),
        };
        let mut required_exports = vec![RuntimeExport::new(package.identity(startup.0), startup.1)];
        if (target.os(), target.arch()) == (Os::Windows, Arch::X86_64) {
            required_exports.push(RuntimeExport::new(
                package.identity("builtins/windows/x86_64.nia"),
                "__chkstk",
            ));
        }
        Self::source_from_package(package, target, required_exports)
    }

    /// Builds a source runtime with an explicit native export contract.
    /// Alternate runtime implementations can omit exports supplied only by
    /// the installed platform runtime.
    pub fn source_from_package(
        package: &ToolchainPackage,
        target: &TargetConfig,
        required_exports: impl IntoIterator<Item = RuntimeExport>,
    ) -> Result<Self, RuntimeSpecError> {
        // The startup module is named by architecture below its OS directory.
        match (target.os(), target.arch()) {
            (Os::Linux, Arch::X86_64 | Arch::X86 | Arch::Aarch64)
            | (Os::Windows, Arch::X86_64)
            | (Os::Macos, Arch::X86_64 | Arch::Aarch64) => {}
            _ => return Err(RuntimeSpecError::UnsupportedTarget(*target)),
        }
        let implementation = target.arch().name();
        let (definition_name, linker_symbol) = if target.os() == Os::Macos {
            ("main", "_main")
        } else {
            ("_start", "_start")
        };
        Ok(Self::Source(Box::new(SourceRuntimeSpec {
            package: package.clone(),
            entry_point: RuntimeEntryPoint {
                module_identity: package.identity(&format!(
                    "start/freestanding/{}/{implementation}.nia",
                    target.os().name()
                )),
                definition_name: definition_name.to_string(),
                linker_symbol: linker_symbol.to_string(),
            },
            required_exports: required_exports.into_iter().collect(),
            target: *target,
        })))
    }

    /// Validates that this runtime can participate in a request for `target`.
    pub fn validate_for_target(&self, target: &TargetConfig) -> Result<(), RuntimeSpecError> {
        let Self::Source(runtime) = self else {
            return Ok(());
        };
        if runtime.target != *target {
            return Err(RuntimeSpecError::TargetMismatch {
                runtime: runtime.target,
                request: *target,
            });
        }
        Ok(())
    }
}

/// Failure to select a runtime implementation for a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSpecError {
    /// The installed runtime has no implementation for this target.
    UnsupportedTarget(TargetConfig),
    /// A validated runtime was reused for a different compilation target.
    TargetMismatch {
        runtime: TargetConfig,
        request: TargetConfig,
    },
}

impl fmt::Display for RuntimeSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTarget(target) => {
                write!(f, "no freestanding runtime for target `{target}`")
            }
            Self::TargetMismatch { runtime, request } => write!(
                f,
                "runtime target `{runtime}` does not match request target `{request}`"
            ),
        }
    }
}

impl std::error::Error for RuntimeSpecError {}

/// A source package shipped under the resource root.
///
/// It is an ordinary package: its `nia.toml` names the root file through
/// `[library].path`. Its sources are identified below `toolchain:/<name>/`, so
/// identities survive relocating the installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainPackage {
    name: String,
    directory: PathBuf,
    library_path: String,
}

impl ToolchainPackage {
    /// Describes a package from its directory and manifest library path.
    pub fn new(
        name: impl Into<String>,
        directory: impl Into<PathBuf>,
        library_path: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            directory: directory.into(),
            library_path: library_path.into(),
        }
    }

    /// Canonical package identity owning the package's linkage.
    pub fn id(&self) -> nia_package_metadata::PackageId {
        nia_package_metadata::PackageId::toolchain(&self.name)
    }

    /// Physical root file of the package's module tree.
    pub fn root_file(&self) -> PathBuf {
        self.directory.join(&self.library_path)
    }

    /// Relocation-independent identity of a package-relative source path.
    pub fn identity(&self, relative: &str) -> String {
        format!("toolchain:/{}/{relative}", self.name)
    }

    /// Relocation-independent identity of the root file.
    pub fn root_identity(&self) -> String {
        self.identity(&self.library_path)
    }

    /// Relocation-independent identity of the package directory, which
    /// scopes the package's dependency names.
    pub fn directory_identity(&self) -> String {
        format!("toolchain:/{}", self.name)
    }
}

/// Validated compiler executable, resources, identity, and target configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainLayout {
    compiler_executable: PathBuf,
    resource_root: PathBuf,
    std: ToolchainPackage,
    identity: ToolchainIdentity,
    host_target: TargetConfig,
    artifact_target: TargetConfig,
    runtime: ToolchainPackage,
    bundled_lld: Option<PathBuf>,
}

impl ToolchainLayout {
    /// Resolves and validates every resource named by `request`.
    ///
    /// The resource root is canonicalized before exposure. The manifest must
    /// exactly match this compiler's protocol epoch and compiler
    /// version; required standard-library and runtime files must
    /// be regular files.
    pub fn resolve(request: ToolchainLayoutRequest) -> Result<Self, ToolchainLayoutError> {
        // Build scripts and host tools run on the compiler host, so it must be
        // a maintained target even when artifacts are cross-compiled.
        let host_target = TargetConfig::host().ok_or(ToolchainLayoutError::UnsupportedHost {
            arch: std::env::consts::ARCH,
            os: std::env::consts::OS,
        })?;
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

        let std = read_toolchain_package(&resource_root, "std", ResourceRole::StandardLibrary)?;
        let runtime =
            read_toolchain_package(&resource_root, "runtime", ResourceRole::FreestandingRuntime)?;
        let bundled_lld = match resource_root
            .parent()
            .map(|prefix| prefix.join(bundled_lld_path()))
            .filter(|path| path.exists())
        {
            Some(path) => Some(validate_optional_file(&path, ResourceRole::BundledLinker)?),
            None => None,
        };

        Ok(Self {
            compiler_executable: request.compiler_executable,
            resource_root,
            std,
            identity,
            host_target,
            artifact_target: request.artifact_target.unwrap_or(host_target),
            runtime,
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

    /// Returns the validated standard-library package.
    pub fn std_package(&self) -> &ToolchainPackage {
        &self.std
    }

    /// Returns the validated private source package for freestanding startup.
    ///
    /// Runtime startup is a private toolchain source package, not a module
    /// owned by the standard library.
    pub fn freestanding_runtime_package(&self) -> &ToolchainPackage {
        &self.runtime
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

    /// Returns the bundled LLD executable when the resource tree provides one.
    pub fn bundled_lld(&self) -> Option<&std::path::Path> {
        self.bundled_lld.as_deref()
    }

    /// Reads the import descriptions of the system libraries that the
    /// target's standard library and runtime import, in link order.
    ///
    /// A description lists only the symbols the toolchain sources use, so a
    /// link needs no platform SDK import library.
    pub fn import_descriptions(
        &self,
        target: TargetConfig,
    ) -> Result<Vec<ImportDescription>, ToolchainLayoutError> {
        let (directory, libraries, extension): (&str, &[&str], &str) = match target.os() {
            Os::Windows => ("windows", &["kernel32", "bcryptprimitives"], "def"),
            Os::Linux => return Ok(Vec::new()),
            Os::Macos => ("macos", &["libSystem"], "tbd"),
        };
        libraries
            .iter()
            .map(|library| {
                let path = self
                    .resource_root
                    .join("imports")
                    .join(directory)
                    .join(format!("{library}.{extension}"));
                validate_file(&path, ResourceRole::ImportDescription)?;
                let contents = read_bounded_resource(&path).map_err(|error| {
                    ToolchainLayoutError::ReadResource {
                        role: ResourceRole::ImportDescription,
                        path: path.clone(),
                        error,
                    }
                })?;
                Ok(ImportDescription {
                    library: (*library).to_string(),
                    contents,
                })
            })
            .collect()
    }
}

/// A system library's import description shipped with the toolchain, such as
/// `imports/windows/kernel32.def` or `imports/macos/libSystem.tbd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDescription {
    library: String,
    contents: Vec<u8>,
}

impl ImportDescription {
    /// Library name, without platform prefix or extension.
    pub fn library(&self) -> &str {
        &self.library
    }

    /// Description contents in the platform's format.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }
}

fn read_bounded_resource(path: &std::path::Path) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut contents = Vec::new();
    file.take((MAX_RESOURCE_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut contents)?;
    if contents.len() > MAX_RESOURCE_MANIFEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("toolchain resource exceeds the {MAX_RESOURCE_MANIFEST_BYTES}-byte limit"),
        ));
    }
    Ok(contents)
}

// The bundled LLD runs on the host, so it carries the host executable suffix.
fn bundled_lld_path() -> String {
    format!("libexec/ld.lld{}", std::env::consts::EXE_SUFFIX)
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
    /// Explicit artifact target; `None` compiles for the host.
    artifact_target: Option<TargetConfig>,
}

impl ToolchainLayoutRequest {
    /// Selects resources at `../lib` relative to the executable directory.
    pub fn installed(compiler_executable: impl Into<PathBuf>) -> Self {
        Self {
            compiler_executable: compiler_executable.into(),
            resources: ResourceRootSelection::Installed,
            artifact_target: None,
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
            artifact_target: None,
        }
    }

    /// Overrides the artifact target while leaving compiler tools on the host target.
    pub fn with_artifact_target(mut self, target: TargetConfig) -> Self {
        self.artifact_target = Some(target);
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
    /// System library import description for a target.
    ImportDescription,
}

impl fmt::Display for ResourceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CompilerExecutable => "compiler executable",
            Self::ResourceRoot => "toolchain resource root",
            Self::StandardLibrary => "standard-library package",
            Self::FreestandingRuntime => "freestanding runtime package",
            Self::BundledLinker => "bundled LLD linker",
            Self::ImportDescription => "system import description",
        })
    }
}

/// Failure to discover, parse, or validate a toolchain layout.
#[derive(Debug)]
pub enum ToolchainLayoutError {
    /// The compiler runs on a host outside the maintained target matrix.
    UnsupportedHost {
        /// Host architecture reported by the Rust runtime.
        arch: &'static str,
        /// Host operating system reported by the Rust runtime.
        os: &'static str,
    },
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
    /// A shipped source package's manifest is unreadable or names another package.
    InvalidPackage {
        /// Package being validated.
        role: ResourceRole,
        /// Manifest path.
        path: PathBuf,
        /// Description of the problem.
        message: String,
    },
}

impl fmt::Display for ToolchainLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHost { arch, os } => write!(
                f,
                "compiler host `{arch}-{os}` is not a maintained Nia target"
            ),
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
            Self::InvalidPackage {
                role,
                path,
                message,
            } => write!(f, "invalid {role} manifest `{}`: {message}", path.display()),
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

/// Reads a shipped package's manifest and validates its root file.
///
/// The manifest must name the package `nia/<name>` at this compiler's version
/// and declare a `[library].path`; nothing about the root file name is assumed.
fn read_toolchain_package(
    resource_root: &std::path::Path,
    name: &str,
    role: ResourceRole,
) -> Result<ToolchainPackage, ToolchainLayoutError> {
    let directory = resource_root.join(name);
    let manifest_path = directory.join("nia.toml");
    validate_file(&manifest_path, role)?;
    let invalid = |message: String| ToolchainLayoutError::InvalidPackage {
        role,
        path: manifest_path.clone(),
        message,
    };
    let manifest = nia_package_metadata::PackageManifest::read_standalone(&directory)
        .map_err(|error| invalid(error.to_string()))?;
    let package = &manifest.package;
    if package.namespace != "nia" || package.name != name {
        return Err(invalid(format!(
            "package must be `nia/{name}`, found `{}/{}`",
            package.namespace, package.name
        )));
    }
    if package.version.to_string() != COMPILER_VERSION {
        return Err(invalid(format!(
            "package version must be the compiler version `{COMPILER_VERSION}`, found `{}`",
            package.version
        )));
    }
    let library_path = manifest
        .library_path
        .ok_or_else(|| invalid("package must declare `[library].path`".to_string()))?;
    let package = ToolchainPackage::new(name, directory, library_path);
    validate_file(&package.root_file(), role)?;
    Ok(package)
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
    protocol_epoch: Option<ManifestValue>,
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
            "protocol-epoch" => &mut fields.protocol_epoch,
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
        protocol_epoch: parse_u32(
            path,
            "protocol-epoch",
            required(path, "protocol-epoch", fields.protocol_epoch)?,
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
        "protocol-epoch",
        PROTOCOL_EPOCH.to_string(),
        identity.protocol_epoch.to_string(),
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
        for name in nia_compat::TOOLCHAIN_PACKAGES {
            fs::write(
                resources.join(name).join("nia.toml"),
                nia_compat::toolchain_package_manifest(name),
            )
            .expect("write package manifest");
        }
        fs::write(resources.join("std/root.nia"), "pub module start;").expect("write std root");
        fs::write(
            resources.join("runtime/root.nia"),
            "pub(root) module start;",
        )
        .expect("write runtime root");
        fs::write(resources.join("runtime/start.nia"), "").expect("write runtime");
        executable
    }

    #[test]
    fn import_descriptions_are_target_owned_resources() {
        let root = temp_dir("import_descriptions");
        let executable = write_layout(&root);
        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("layout");
        let windows = TargetConfig::parse("x86_64-pc-windows-msvc").unwrap();
        assert!(matches!(
            layout.import_descriptions(windows),
            Err(ToolchainLayoutError::MissingResource {
                role: ResourceRole::ImportDescription,
                ..
            })
        ));

        let imports = root.join("lib/imports/windows");
        fs::create_dir_all(&imports).expect("create import descriptions");
        fs::write(imports.join("kernel32.def"), b"LIBRARY kernel32.dll\n").expect("write");
        fs::write(
            imports.join("bcryptprimitives.def"),
            b"LIBRARY bcryptprimitives.dll\n",
        )
        .expect("write");
        let descriptions = layout.import_descriptions(windows).expect("descriptions");
        assert_eq!(
            descriptions
                .iter()
                .map(|description| (description.library(), description.contents()))
                .collect::<Vec<_>>(),
            [
                ("kernel32", &b"LIBRARY kernel32.dll\n"[..]),
                ("bcryptprimitives", &b"LIBRARY bcryptprimitives.dll\n"[..]),
            ]
        );
        let mac_imports = root.join("lib/imports/macos");
        fs::create_dir_all(&mac_imports).expect("create Darwin import descriptions");
        fs::write(
            mac_imports.join("libSystem.tbd"),
            b"--- !tapi-tbd\ntbd-version:     4\ntargets:         [ arm64-macos, x86_64-macos ]\nflags:           [ not_app_extension_safe ]\ninstall-name:    '/usr/lib/libSystem.B.dylib'\ncurrent-version: 0\ncompatibility-version: 0\nexports:\n  - targets:         [ arm64-macos, x86_64-macos ]\n    symbols:        [ _exit, _write ]\n...\n",
        )
        .expect("write Darwin import description");
        let descriptions = layout
            .import_descriptions(TargetConfig::parse("aarch64-apple-macos").unwrap())
            .expect("Darwin descriptions");
        assert_eq!(
            descriptions
                .iter()
                .map(|description| (description.library(), description.contents()))
                .collect::<Vec<_>>(),
            [(
                "libSystem",
                &b"--- !tapi-tbd\ntbd-version:     4\ntargets:         [ arm64-macos, x86_64-macos ]\nflags:           [ not_app_extension_safe ]\ninstall-name:    '/usr/lib/libSystem.B.dylib'\ncurrent-version: 0\ncompatibility-version: 0\nexports:\n  - targets:         [ arm64-macos, x86_64-macos ]\n    symbols:        [ _exit, _write ]\n...\n"[..]
            )]
        );
        let name = "x86_64-unknown-linux";
        let target = TargetConfig::parse(name).unwrap();
        assert!(
            layout.import_descriptions(target).unwrap().is_empty(),
            "{name}"
        );
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
        let relocated_root = temp_dir("relocated_layout");
        fs::rename(&first, relocated_root.join("toolchain")).expect("relocate toolchain");
        let relocated = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(
            relocated_root.join("toolchain/bin/nia"),
        ))
        .expect("relocated layout");
        assert_eq!(relocated.identity(), installed.identity());
        assert_ne!(relocated.resource_root(), installed.resource_root());
        assert_eq!(
            relocated.std_package().root_file(),
            relocated.resource_root().join("std/root.nia")
        );
    }

    #[test]
    fn freestanding_runtime_has_stable_source_and_abi_identities() {
        let root = temp_dir("freestanding_runtime_spec");
        let executable = write_layout(&root);
        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("installed layout");
        for (name, implementation) in [
            ("x86_64-unknown-linux", "x86_64"),
            ("x86-unknown-linux", "x86"),
            ("aarch64-unknown-linux", "aarch64"),
            ("x86_64-pc-windows-msvc", "x86_64"),
            ("x86_64-apple-macos", "x86_64"),
            ("aarch64-apple-macos", "aarch64"),
        ] {
            let target = TargetConfig::parse(name).expect("maintained target");
            let runtime = RuntimeSpec::freestanding(&layout, &target).expect("runtime target");
            let source = runtime.source().expect("source runtime");

            let package = source.package();
            assert_eq!(package.root_identity(), "toolchain:/runtime/root.nia");
            assert_eq!(package.directory_identity(), "toolchain:/runtime");
            assert_eq!(
                package.id(),
                nia_package_metadata::PackageId::toolchain("runtime")
            );
            let expected_runtime_root = root
                .join("lib/runtime/root.nia")
                .canonicalize()
                .unwrap_or_else(|_| root.join("lib/runtime/root.nia"));
            assert_eq!(package.root_file(), expected_runtime_root);
            assert_eq!(
                source.entry_point().module_identity(),
                format!(
                    "toolchain:/runtime/start/freestanding/{}/{implementation}.nia",
                    target.os().name()
                )
            );
            let expected_definition = if target.os() == Os::Macos {
                "main"
            } else {
                "_start"
            };
            let expected_linker_symbol = if target.os() == Os::Macos {
                "_main"
            } else {
                "_start"
            };
            assert_eq!(source.entry_point().definition_name(), expected_definition);
            assert_eq!(source.entry_point().linker_symbol(), expected_linker_symbol);
            if target.os() == Os::Macos {
                assert_eq!(
                    source.required_exports(),
                    [RuntimeExport::new(
                        "toolchain:/runtime/start/freestanding/macos/startup.nia",
                        "niaDarwinStart",
                    )]
                );
            } else if target.os() == Os::Windows {
                assert_eq!(
                    source.required_exports(),
                    [
                        RuntimeExport::new(
                            "toolchain:/runtime/start/freestanding/windows/startup.nia",
                            "niaWindowsStart",
                        ),
                        RuntimeExport::new(
                            "toolchain:/runtime/builtins/windows/x86_64.nia",
                            "__chkstk",
                        ),
                    ]
                );
            } else {
                assert_eq!(
                    source.required_exports(),
                    [RuntimeExport::new(
                        "toolchain:/runtime/start/freestanding/linux/startup.nia",
                        "niaStartStack",
                    )]
                );
            }
            assert_eq!(source.target(), &target);
            assert_eq!(runtime.validate_for_target(&target), Ok(()));
        }
    }

    #[test]
    fn runtime_selection_rejects_unknown_and_mismatched_targets() {
        let unsupported = TargetConfig::parse("aarch64-pc-windows-msvc").unwrap_err();
        assert_eq!(unsupported.name, "aarch64-pc-windows-msvc");

        let target = TargetConfig::parse("x86_64-unknown-linux").unwrap();
        let package = ToolchainPackage::new("runtime", "runtime", "root.nia");
        let runtime = RuntimeSpec::freestanding_from_package(&package, &target).unwrap();
        let different = TargetConfig::parse("x86-unknown-linux").unwrap();
        assert!(matches!(
            runtime.validate_for_target(&different),
            Err(RuntimeSpecError::TargetMismatch { .. })
        ));
    }

    #[test]
    fn exposes_bundled_lld_from_release_layout() {
        let root = temp_dir("bundled_lld");
        let executable = write_layout(&root);
        let lld = root.join(bundled_lld_path());
        fs::create_dir_all(lld.parent().expect("lld parent")).expect("create lld directory");
        fs::write(&lld, b"lld").expect("write bundled lld");

        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("release layout");
        assert_eq!(layout.bundled_lld(), lld.canonicalize().ok().as_deref());
    }

    #[test]
    fn rejects_missing_and_incompatible_resources() {
        let root = temp_dir("rejects_resources");
        let executable = write_layout(&root);
        fs::write(
            root.join("lib/toolchain.meta"),
            "protocol-epoch=1\ncompiler-version=incompatible\n",
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
    fn toolchain_packages_are_rooted_where_their_manifest_says() {
        let root = temp_dir("package_manifests");
        let executable = write_layout(&root);
        let manifest = root.join("lib/std/nia.toml");

        // The root file is whatever `[library].path` names.
        fs::rename(root.join("lib/std/root.nia"), root.join("lib/std/core.nia"))
            .expect("rename std root");
        fs::write(
            &manifest,
            nia_compat::toolchain_package_manifest("std").replace("root.nia", "core.nia"),
        )
        .expect("point std at its new root");
        let layout = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect("layout");
        assert_eq!(
            layout.std_package().root_file(),
            layout.resource_root().join("std/core.nia")
        );
        assert_eq!(
            layout.std_package().root_identity(),
            "toolchain:/std/core.nia"
        );

        for (contents, expected) in [
            (
                nia_compat::toolchain_package_manifest("other"),
                "must be `nia/std`",
            ),
            (
                nia_compat::toolchain_package_manifest("std").replace(COMPILER_VERSION, "0.0.1"),
                "compiler version",
            ),
            (
                "[package]\nnamespace = \"nia\"\nname = \"std\"\nversion = \"0.0.0\"\n"
                    .replace("0.0.0", COMPILER_VERSION),
                "`[library].path`",
            ),
        ] {
            fs::write(&manifest, contents).expect("write manifest");
            let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
                .expect_err("invalid std manifest");
            assert!(
                matches!(error, ToolchainLayoutError::InvalidPackage { .. }),
                "{error}"
            );
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn malformed_numeric_manifest_field_reports_its_source_line() {
        let path = PathBuf::from("toolchain.meta");
        let manifest =
            format!("# identity\ncompiler-version={COMPILER_VERSION}\nprotocol-epoch=invalid\n");
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
            protocol_epoch: 1,
        };
        let baseline_fingerprint = baseline.fingerprint();
        for changed in [
            ToolchainIdentity {
                compiler_version: "changed".to_string(),
                ..baseline.clone()
            },
            ToolchainIdentity {
                protocol_epoch: 9,
                ..baseline.clone()
            },
        ] {
            assert_ne!(baseline_fingerprint, changed.fingerprint());
        }

        assert_eq!(
            ToolchainIdentityFingerprint::current(),
            ToolchainIdentity {
                compiler_version: COMPILER_VERSION.to_string(),
                protocol_epoch: PROTOCOL_EPOCH,
            }
            .fingerprint()
        );
    }
}
