// SPDX-License-Identifier: GPL-3.0-or-later
//! Toolchain resource discovery and compatibility identity.
//!
//! A layout binds one compiler executable to a canonical resource root, a
//! versioned manifest, the standard library, and runtime startup modules.
//! Compatibility identity deliberately excludes filesystem paths so an intact
//! installation can be relocated without invalidating compiler caches.

use nia_compat::{COMPILER_VERSION, toolchain};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};
use nia_target_config::TargetConfig;
use std::{fmt, fs, io, io::Read, path::PathBuf};

const COMPATIBILITY_IDENTITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.toolchain.compatibility-identity.v1");
const MAX_RESOURCE_MANIFEST_BYTES: usize = 64 * 1024;

/// File name of the versioned compatibility manifest under a resource root.
pub const RESOURCE_MANIFEST_NAME: &str = "toolchain.meta";

/// Path-independent compatibility identity read from `toolchain.meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainIdentity {
    compiler_version: String,
    resource_layout_schema: u32,
    std_schema: u32,
    build_protocol_schema: u32,
}

/// Stable fingerprint of all fields in [`ToolchainIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolchainIdentityFingerprint(nia_query::QueryFingerprint);

impl ToolchainIdentityFingerprint {
    /// Returns the identity fingerprint expected by this compiler build.
    pub fn current() -> Self {
        ToolchainIdentity {
            compiler_version: COMPILER_VERSION.to_string(),
            resource_layout_schema: toolchain::RESOURCE_LAYOUT,
            std_schema: toolchain::STANDARD_LIBRARY,
            build_protocol_schema: toolchain::BUILD_PROTOCOL,
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

    /// Returns the resource directory layout schema version.
    pub const fn resource_layout_schema(&self) -> u32 {
        self.resource_layout_schema
    }

    /// Returns the standard-library source schema version.
    pub const fn std_schema(&self) -> u32 {
        self.std_schema
    }

    /// Returns the build runner protocol schema version.
    pub const fn build_protocol_schema(&self) -> u32 {
        self.build_protocol_schema
    }

    /// Computes a deterministic fingerprint over every compatibility field.
    pub fn fingerprint(&self) -> ToolchainIdentityFingerprint {
        let mut builder = QueryFingerprintBuilder::new(COMPATIBILITY_IDENTITY_DOMAIN);
        builder.write_str(&self.compiler_version);
        builder.write_u64(u64::from(self.resource_layout_schema));
        builder.write_u64(u64::from(self.std_schema));
        builder.write_u64(u64::from(self.build_protocol_schema));
        ToolchainIdentityFingerprint(builder.finish())
    }
}

/// Runtime source modules shipped in the resolved resource bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResources {
    freestanding_start_module: PathBuf,
}

impl RuntimeResources {
    /// Returns the startup module used for freestanding executables.
    pub fn freestanding_start_module(&self) -> &std::path::Path {
        &self.freestanding_start_module
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
}

impl ToolchainLayout {
    /// Resolves and validates every resource named by `request`.
    ///
    /// The resource root is canonicalized before exposure. The manifest must
    /// exactly match this compiler's layout, standard-library, build-protocol,
    /// and compiler versions; required standard-library and runtime files must
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
                .join("../lib/nia"),
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
        let freestanding_start_module = resource_root.join("std/start.nia");
        validate_file(
            &freestanding_start_module,
            ResourceRole::FreestandingRuntime,
        )?;

        Ok(Self {
            compiler_executable: request.compiler_executable,
            resource_root,
            std_module,
            identity,
            host_target: TargetConfig::host(),
            artifact_target: request.artifact_target,
            runtime: RuntimeResources {
                freestanding_start_module,
            },
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
    /// Selects resources at `../lib/nia` relative to the executable directory.
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
}

impl fmt::Display for ResourceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CompilerExecutable => "compiler executable",
            Self::ResourceRoot => "toolchain resource root",
            Self::StandardLibrary => "standard-library root module",
            Self::FreestandingRuntime => "freestanding runtime module",
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

#[derive(Default)]
struct ManifestFields {
    resource_layout_schema: Option<ManifestValue>,
    compiler_version: Option<ManifestValue>,
    std_schema: Option<ManifestValue>,
    build_protocol_schema: Option<ManifestValue>,
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
            "resource-layout-schema" => &mut fields.resource_layout_schema,
            "compiler-version" => &mut fields.compiler_version,
            "std-schema" => &mut fields.std_schema,
            "build-protocol-schema" => &mut fields.build_protocol_schema,
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
        resource_layout_schema: parse_u32(
            path,
            "resource-layout-schema",
            required(
                path,
                "resource-layout-schema",
                fields.resource_layout_schema,
            )?,
        )?,
        std_schema: parse_u32(
            path,
            "std-schema",
            required(path, "std-schema", fields.std_schema)?,
        )?,
        build_protocol_schema: parse_u32(
            path,
            "build-protocol-schema",
            required(path, "build-protocol-schema", fields.build_protocol_schema)?,
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
        "resource-layout-schema",
        toolchain::RESOURCE_LAYOUT.to_string(),
        identity.resource_layout_schema.to_string(),
    )?;
    validate_field(
        path,
        "compiler-version",
        COMPILER_VERSION.to_string(),
        identity.compiler_version.clone(),
    )?;
    validate_field(
        path,
        "std-schema",
        toolchain::STANDARD_LIBRARY.to_string(),
        identity.std_schema.to_string(),
    )?;
    validate_field(
        path,
        "build-protocol-schema",
        toolchain::BUILD_PROTOCOL.to_string(),
        identity.build_protocol_schema.to_string(),
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
        let resources = root.join("lib/nia");
        fs::create_dir_all(resources.join("std")).expect("create std directory");
        fs::write(
            resources.join(RESOURCE_MANIFEST_NAME),
            nia_compat::toolchain_manifest(),
        )
        .expect("write manifest");
        fs::write(resources.join("std/pkg.nia"), "pub module start;").expect("write std root");
        fs::write(resources.join("std/start.nia"), "").expect("write runtime");
        executable
    }

    #[test]
    fn resolves_explicit_and_installed_layouts_with_path_independent_identity() {
        let first = temp_dir("resolves_layout");
        let executable = write_layout(&first);
        let explicit = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            &executable,
            first.join("lib/nia"),
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
    }

    #[test]
    fn rejects_missing_and_incompatible_resources() {
        let root = temp_dir("rejects_resources");
        let executable = write_layout(&root);
        fs::write(
            root.join("lib/nia/toolchain.meta"),
            "resource-layout-schema=1\ncompiler-version=incompatible\nstd-schema=1\nbuild-protocol-schema=3\n",
        )
        .expect("replace manifest");
        let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect_err("incompatible compiler version");
        assert!(error.to_string().contains("`compiler-version`"), "{error}");

        fs::remove_file(root.join("lib/nia/toolchain.meta")).expect("remove manifest");
        let error = ToolchainLayout::resolve(ToolchainLayoutRequest::installed(&executable))
            .expect_err("missing manifest");
        assert!(error.to_string().contains("resource manifest"), "{error}");
    }

    #[test]
    fn malformed_numeric_manifest_field_reports_its_source_line() {
        let path = PathBuf::from("toolchain.meta");
        let manifest = format!(
            "# identity\ncompiler-version={COMPILER_VERSION}\nresource-layout-schema=invalid\nstd-schema=1\nbuild-protocol-schema=3\n"
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
        let manifest = root.join("lib/nia/toolchain.meta");
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
            resource_layout_schema: 1,
            std_schema: 2,
            build_protocol_schema: 3,
        };
        let baseline_fingerprint = baseline.fingerprint();
        for changed in [
            ToolchainIdentity {
                compiler_version: "changed".to_string(),
                ..baseline.clone()
            },
            ToolchainIdentity {
                resource_layout_schema: 9,
                ..baseline.clone()
            },
            ToolchainIdentity {
                std_schema: 9,
                ..baseline.clone()
            },
            ToolchainIdentity {
                build_protocol_schema: 9,
                ..baseline.clone()
            },
        ] {
            assert_ne!(baseline_fingerprint, changed.fingerprint());
        }

        assert_eq!(
            ToolchainIdentityFingerprint::current(),
            ToolchainIdentity {
                compiler_version: COMPILER_VERSION.to_string(),
                resource_layout_schema: toolchain::RESOURCE_LAYOUT,
                std_schema: toolchain::STANDARD_LIBRARY,
                build_protocol_schema: toolchain::BUILD_PROTOCOL,
            }
            .fingerprint()
        );
    }
}
