// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed linker/archive invocation and incremental result identity.
//!
//! Cache identity is split into independently diagnosable input, toolchain,
//! target, tool, and option components. Executable links are cacheable only
//! when every external input can be represented by those components; opaque
//! sysroots, native libraries, and raw arguments deliberately disable reuse.

use std::{
    collections::HashSet,
    env, fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use nia_backend_ir::{CodegenUnitKey, IncrementalLinkInputs};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};
use nia_target_config::TargetConfig;

const LINK_RESULT_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-components.v2");
const ARCHIVE_RESULT_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.archive-result-components.v1");
const ARCHIVE_TOOLCHAIN_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.archive-toolchain.v1");
const ARCHIVE_TARGET_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.archive-target.v1");
const ARCHIVE_TOOL_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.archive-tool.v1");
const ARCHIVE_OPTIONS_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.archive-options.v1");
const ARCHIVE_CACHE_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.archive-result-cache-key.v1");
const ARCHIVE_INPUTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.archive-result-inputs.v1");
const STATIC_ARCHIVE_LINK_INPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.static-archive-link-input.v1");
const LINK_RESULT_CACHE_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-cache-key.v2");
const LINK_RESULT_INPUTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-inputs.v2");
const LINK_RESULT_TOOLCHAIN_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-toolchain.v1");
const LINK_RESULT_TARGET_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-target.v2");
const LINK_RESULT_LINKER_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-linker.v2");
const LINK_RESULT_OPTIONS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-options.v2");
const MAX_LD_SO_CONF_FILE_BYTES: usize = 1024 * 1024;
const MAX_LD_SO_CONF_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_LD_SO_CONF_FILES: usize = 1024;
const MAX_LD_SO_CONF_INCLUDE_ENTRIES: usize = 4096;

/// Stable identity for an executable link result or one of its components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkResultFingerprint([u64; 2]);

impl LinkResultFingerprint {
    /// Reconstructs a persisted fingerprint from two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the two lanes for persistence.
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

/// Stable logical link key derived from ordered codegen-unit identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkResultCacheKey([u64; 2]);

impl LinkResultCacheKey {
    /// Reconstructs a persisted cache key from two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the two lanes for persistence and cache paths.
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

/// Independently comparable components of executable link identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultFingerprintComponents {
    /// Ordered codegen-unit and static-archive identities and contents.
    pub inputs: LinkResultFingerprint,
    /// Compiler/toolchain compatibility identity.
    pub toolchain: LinkResultFingerprint,
    /// Target triple, dynamic loader, and implicit target search paths.
    pub target: LinkResultFingerprint,
    /// Canonical linker path, bytes, and flavor.
    pub linker: LinkResultFingerprint,
    /// Structured invocation options owned by this crate.
    pub options: LinkResultFingerprint,
}

/// Complete cache identity for one executable link result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultFingerprintSet {
    /// Logical key shared by revisions of the same ordered input identities.
    pub cache_key: LinkResultCacheKey,
    /// Combined identity of every component.
    pub fingerprint: LinkResultFingerprint,
    /// Components retained for precise invalidation reporting.
    pub components: LinkResultFingerprintComponents,
}

/// Non-input components used to validate a previously described link environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultEnvironmentFingerprint {
    /// Toolchain identity component.
    pub toolchain: LinkResultFingerprint,
    /// Target environment component.
    pub target: LinkResultFingerprint,
    /// Linker executable component.
    pub linker: LinkResultFingerprint,
    /// Structured options component.
    pub options: LinkResultFingerprint,
}

impl LinkResultFingerprintSet {
    /// Combines a logical cache key and all independently stored components.
    pub fn new(cache_key: LinkResultCacheKey, components: LinkResultFingerprintComponents) -> Self {
        let mut builder = QueryFingerprintBuilder::new(LINK_RESULT_FINGERPRINT_DOMAIN);
        for component in [
            components.inputs,
            components.toolchain,
            components.target,
            components.linker,
            components.options,
        ] {
            for part in component.parts() {
                builder.write_u64(part);
            }
        }
        Self {
            cache_key,
            fingerprint: LinkResultFingerprint::from_parts(builder.finish().parts()),
            components,
        }
    }
}

/// Component-level reasons why a cached executable link result is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultInvalidation {
    /// Whether typed inputs changed.
    pub inputs: bool,
    /// Whether toolchain compatibility changed.
    pub toolchain: bool,
    /// Whether the target environment changed.
    pub target: bool,
    /// Whether linker identity changed.
    pub linker: bool,
    /// Whether structured options changed.
    pub options: bool,
}

impl LinkResultInvalidation {
    /// Compares cached and expected components.
    pub fn between(
        cached: LinkResultFingerprintComponents,
        expected: LinkResultFingerprintComponents,
    ) -> Self {
        Self {
            inputs: cached.inputs != expected.inputs,
            toolchain: cached.toolchain != expected.toolchain,
            target: cached.target != expected.target,
            linker: cached.linker != expected.linker,
            options: cached.options != expected.options,
        }
    }

    /// Returns the number of changed components.
    pub fn count(self) -> u32 {
        u32::from(self.inputs)
            + u32::from(self.toolchain)
            + u32::from(self.target)
            + u32::from(self.linker)
            + u32::from(self.options)
    }
}

/// Command-line protocol implemented by an executable linker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    /// GNU `ld` compatible command line.
    Gnu,
    /// LLVM `ld.lld` using the GNU-compatible command line.
    Lld,
    /// Reserved future in-process ELF linker.
    SelfHostedElf,
}

/// Whether an executable link is fully static or permits dynamic dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Request a static executable.
    Static,
    /// Request a dynamically loadable executable.
    Dynamic,
}

/// Dynamic loader selection for a dynamic executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicLinker {
    /// Derive the standard loader from the target or native executable metadata.
    Auto,
    /// Explicitly request no dynamic loader.
    None,
    /// Use the exact loader path.
    Path(String),
}

/// Per-library override for static or dynamic search behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryLinkMode {
    /// Follow the enclosing executable link mode.
    Default,
    /// Search only static libraries for this and following libraries.
    Static,
    /// Search only dynamic libraries for this and following libraries.
    Dynamic,
}

/// One native library name and its search-mode selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLibrary {
    /// Linker library name, without the `-l` prefix.
    pub name: String,
    /// Search mode applied before this library.
    pub mode: LibraryLinkMode,
}

impl NativeLibrary {
    /// Creates a library following the enclosing link mode.
    pub fn default(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Default,
        }
    }

    /// Creates a library selected from static archives.
    pub fn static_(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Static,
        }
    }

    /// Creates a library selected from dynamic libraries.
    pub fn dynamic(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Dynamic,
        }
    }
}

/// Executable linker program and command-line flavor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableLinker {
    /// Program name or path; an empty LLD program triggers discovery.
    pub program: String,
    /// Command-line protocol used by the program.
    pub flavor: LinkerFlavor,
}

impl ExecutableLinker {
    /// Selects `NIA_LINKER` when set, otherwise GNU `ld`.
    pub fn native() -> Self {
        if let Ok(program) = env::var("NIA_LINKER")
            && !program.is_empty()
        {
            return Self::with_program(program);
        }
        Self::with_program("ld")
    }

    /// Selects an explicit GNU-compatible linker program.
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            flavor: LinkerFlavor::Gnu,
        }
    }

    /// Selects an explicit program and command-line flavor.
    pub fn with_program_and_flavor(program: impl Into<String>, flavor: LinkerFlavor) -> Self {
        Self {
            program: program.into(),
            flavor,
        }
    }

    /// Selects discoverable LLD, honoring `NIA_LLD` before `PATH`.
    pub fn lld() -> Self {
        Self {
            program: String::new(),
            flavor: LinkerFlavor::Lld,
        }
    }
}

/// Static archive tool program selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTool {
    /// Explicit program name/path, or empty to discover `llvm-ar` then `ar`.
    pub program: String,
}

impl ArchiveTool {
    /// Selects `NIA_AR` when set, otherwise automatic discovery.
    pub fn native() -> Self {
        if let Ok(program) = env::var("NIA_AR")
            && !program.is_empty()
        {
            return Self::with_program(program);
        }
        Self {
            program: String::new(),
        }
    }

    /// Selects an explicit archive tool program.
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn resolve(&self) -> Result<String, LinkerConfigError> {
        if !self.program.is_empty() {
            return find_program_on_path(&self.program).ok_or_else(|| {
                LinkerConfigError::ArchiveToolNotFound {
                    program: self.program.clone(),
                }
            });
        }
        for program in ["llvm-ar", "ar"] {
            if let Some(found) = find_program_on_path(program) {
                return Ok(found);
            }
        }
        Err(LinkerConfigError::ArchiveToolNotFound {
            program: "llvm-ar or ar".to_string(),
        })
    }
}

/// Stable identity for a static archive result or one of its components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchiveFingerprint([u64; 2]);

impl ArchiveFingerprint {
    /// Reconstructs a persisted fingerprint from two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the two lanes for persistence.
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

/// Stable logical archive key derived from ordered codegen-unit identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchiveCacheKey([u64; 2]);

impl ArchiveCacheKey {
    /// Reconstructs a persisted cache key from two 64-bit lanes.
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    /// Returns the two lanes for persistence and cache paths.
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

/// Independently comparable components of static archive identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFingerprintComponents {
    /// Ordered codegen-unit identities and contents.
    pub inputs: ArchiveFingerprint,
    /// Compiler/toolchain compatibility identity.
    pub toolchain: ArchiveFingerprint,
    /// Target architecture, OS, and ABI.
    pub target: ArchiveFingerprint,
    /// Canonical archive-tool path and bytes.
    pub tool: ArchiveFingerprint,
    /// Deterministic archive protocol and compiler version.
    pub options: ArchiveFingerprint,
}

/// Complete cache identity for one static archive result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFingerprintSet {
    /// Logical key shared by revisions of the same ordered member identities.
    pub cache_key: ArchiveCacheKey,
    /// Combined identity of every component.
    pub fingerprint: ArchiveFingerprint,
    /// Components retained for precise invalidation reporting.
    pub components: ArchiveFingerprintComponents,
}

impl ArchiveFingerprintSet {
    /// Combines a logical cache key and all independently stored components.
    pub fn new(cache_key: ArchiveCacheKey, components: ArchiveFingerprintComponents) -> Self {
        let mut builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_FINGERPRINT_DOMAIN);
        for component in [
            components.inputs,
            components.toolchain,
            components.target,
            components.tool,
            components.options,
        ] {
            for part in component.parts() {
                builder.write_u64(part);
            }
        }
        Self {
            cache_key,
            fingerprint: finish_archive_fingerprint(builder),
            components,
        }
    }
}

/// Component-level reasons why a cached static archive is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveInvalidation {
    /// Whether typed member inputs changed.
    pub inputs: bool,
    /// Whether toolchain compatibility changed.
    pub toolchain: bool,
    /// Whether target identity changed.
    pub target: bool,
    /// Whether archive-tool identity changed.
    pub tool: bool,
    /// Whether deterministic archive options changed.
    pub options: bool,
}

impl ArchiveInvalidation {
    /// Compares cached and expected components.
    pub fn between(
        cached: ArchiveFingerprintComponents,
        expected: ArchiveFingerprintComponents,
    ) -> Self {
        Self {
            inputs: cached.inputs != expected.inputs,
            toolchain: cached.toolchain != expected.toolchain,
            target: cached.target != expected.target,
            tool: cached.tool != expected.tool,
            options: cached.options != expected.options,
        }
    }

    /// Returns the number of changed components.
    pub fn count(self) -> u32 {
        u32::from(self.inputs)
            + u32::from(self.toolchain)
            + u32::from(self.target)
            + u32::from(self.tool)
            + u32::from(self.options)
    }
}

/// Non-input components used to validate a static archive environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveEnvironmentFingerprint {
    /// Toolchain identity component.
    pub toolchain: ArchiveFingerprint,
    /// Target identity component.
    pub target: ArchiveFingerprint,
    /// Archive-tool executable component.
    pub tool: ArchiveFingerprint,
    /// Deterministic options component.
    pub options: ArchiveFingerprint,
}

/// Target and archive-tool selection for deterministic static archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOptions {
    /// Target identity recorded in archive cache entries.
    pub target: LinkTarget,
    /// Archive tool used to materialize the result.
    pub tool: ArchiveTool,
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        Self {
            target: LinkTarget::host(),
            tool: ArchiveTool::native(),
        }
    }
}

impl ArchiveOptions {
    /// Overrides the target recorded in result identity.
    pub fn with_target(mut self, target: LinkTarget) -> Self {
        self.target = target;
        self
    }

    /// Overrides archive-tool discovery.
    pub fn with_tool(mut self, tool: ArchiveTool) -> Self {
        self.tool = tool;
        self
    }

    /// Fingerprints the toolchain, target, resolved tool bytes, and deterministic options.
    pub fn environment_fingerprint(
        &self,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<ArchiveEnvironmentFingerprint, LinkerConfigError> {
        let program = self.tool.resolve()?;
        let program_path =
            PathBuf::from(&program)
                .canonicalize()
                .map_err(|error| LinkerConfigError::Io {
                    path: PathBuf::from(&program),
                    error,
                })?;
        let mut toolchain = QueryFingerprintBuilder::new(ARCHIVE_TOOLCHAIN_DOMAIN);
        for part in toolchain_identity.parts() {
            toolchain.write_u64(part);
        }
        let mut target = QueryFingerprintBuilder::new(ARCHIVE_TARGET_DOMAIN);
        target.write_str(&self.target.arch);
        target.write_str(&self.target.os);
        target.write_str(&self.target.abi);
        let mut tool = QueryFingerprintBuilder::new(ARCHIVE_TOOL_DOMAIN);
        tool.write_str(&program_path.to_string_lossy());
        write_fingerprint_file(&mut tool, &program_path).map_err(|error| {
            LinkerConfigError::Io {
                path: program_path.clone(),
                error,
            }
        })?;
        let mut options = QueryFingerprintBuilder::new(ARCHIVE_OPTIONS_DOMAIN);
        options.write_str(nia_compat::COMPILER_VERSION);
        options.write_str("rcsD");
        Ok(ArchiveEnvironmentFingerprint {
            toolchain: finish_archive_fingerprint(toolchain),
            target: finish_archive_fingerprint(target),
            tool: finish_archive_fingerprint(tool),
            options: finish_archive_fingerprint(options),
        })
    }

    /// Computes complete result identity from ordered typed inputs and the environment.
    pub fn result_fingerprint<T>(
        &self,
        inputs: &IncrementalLinkInputs<T>,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<ArchiveFingerprintSet, LinkerConfigError> {
        let environment = self.environment_fingerprint(toolchain_identity)?;
        let mut cache_key = QueryFingerprintBuilder::new(ARCHIVE_CACHE_KEY_DOMAIN);
        cache_key.write_u64(inputs.len() as u64);
        let mut input_component = QueryFingerprintBuilder::new(ARCHIVE_INPUTS_DOMAIN);
        input_component.write_u64(inputs.len() as u64);
        for input in inputs.as_slice() {
            write_codegen_unit_key(&mut cache_key, &input.key);
            write_codegen_unit_key(&mut input_component, &input.key);
            for part in input.fingerprint.parts() {
                input_component.write_u64(part);
            }
        }
        Ok(ArchiveFingerprintSet::new(
            ArchiveCacheKey::from_parts(cache_key.finish().parts()),
            ArchiveFingerprintComponents {
                inputs: finish_archive_fingerprint(input_component),
                toolchain: environment.toolchain,
                target: environment.target,
                tool: environment.tool,
                options: environment.options,
            },
        ))
    }

    /// Tests whether non-input components still describe the current environment.
    pub fn matches_result_environment(
        &self,
        expected: ArchiveFingerprintComponents,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<bool, LinkerConfigError> {
        let current = self.environment_fingerprint(toolchain_identity)?;
        Ok(current.toolchain == expected.toolchain
            && current.target == expected.target
            && current.tool == expected.tool
            && current.options == expected.options)
    }

    /// Builds a deterministic `rcsD` archive invocation preserving input order.
    pub fn invocation(
        &self,
        inputs: &[PathBuf],
        output: PathBuf,
    ) -> Result<ArchiveInvocation, LinkerConfigError> {
        let program = self.tool.resolve()?;
        let mut args = Vec::with_capacity(inputs.len() + 2);
        args.push("rcsD".to_string());
        args.push(output.to_string_lossy().into_owned());
        args.extend(
            inputs
                .iter()
                .map(|input| input.to_string_lossy().into_owned()),
        );
        Ok(ArchiveInvocation { program, args })
    }
}

/// Fully resolved archive-tool process invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInvocation {
    /// Archive-tool executable path.
    pub program: String,
    /// Ordered command-line arguments.
    pub args: Vec<String>,
}

/// Architecture, operating system, and ABI relevant to link behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    /// Target architecture spelling.
    pub arch: String,
    /// Target operating system spelling.
    pub os: String,
    /// Target ABI spelling, such as `gnu` or `musl`.
    pub abi: String,
}

impl LinkTarget {
    /// Creates the process host target with the platform's default ABI.
    pub fn host() -> Self {
        Self {
            arch: env::consts::ARCH.to_string(),
            os: env::consts::OS.to_string(),
            abi: default_host_abi(),
        }
    }

    /// Converts compiler target configuration, supplying the OS default ABI when absent.
    pub fn from_target_config(config: &TargetConfig) -> Self {
        Self {
            arch: config.arch.clone(),
            os: config.os.clone(),
            abi: if config.abi.is_empty() {
                default_abi_for_os(&config.os)
            } else {
                config.abi.clone()
            },
        }
    }

    /// Tests exact architecture, OS, and ABI equality with the process host.
    pub fn is_host(&self) -> bool {
        let host = Self::host();
        self.arch == host.arch && self.os == host.os && self.abi == host.abi
    }
}

/// Structured executable-link configuration and external inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkOptions {
    /// Target whose executable is produced.
    pub target: LinkTarget,
    /// Executable linker selection.
    pub linker: ExecutableLinker,
    /// Optional entry symbol passed with `-e`.
    pub entry: Option<String>,
    /// Static or dynamic executable mode.
    pub mode: LinkMode,
    /// Dynamic loader policy.
    pub dynamic_linker: DynamicLinker,
    /// Optional external sysroot.
    pub sysroot: Option<String>,
    /// Explicit native library search paths.
    pub library_paths: Vec<String>,
    /// Runtime search paths embedded in the result.
    pub rpaths: Vec<String>,
    /// Native libraries searched by the linker.
    pub libraries: Vec<NativeLibrary>,
    /// Typed static archive inputs with content fingerprints.
    pub static_archives: Vec<StaticArchiveLinkInput>,
    /// Opaque trailing linker arguments.
    pub raw_args: Vec<String>,
}

/// Static archive input split into logical identity, physical path, and content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticArchiveLinkInput {
    package: String,
    name: String,
    path: PathBuf,
    fingerprint: LinkResultFingerprint,
}

impl StaticArchiveLinkInput {
    /// Captures archive content from an in-memory byte slice.
    ///
    /// `package` and `name` participate in logical cache identity; `path` is
    /// used only for invocation, allowing relocation without invalidation.
    pub fn from_bytes(
        package: impl Into<String>,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        bytes: &[u8],
    ) -> Self {
        let mut fingerprint = QueryFingerprintBuilder::new(STATIC_ARCHIVE_LINK_INPUT_DOMAIN);
        fingerprint.write_bytes(bytes);
        Self {
            package: package.into(),
            name: name.into(),
            path: path.into(),
            fingerprint: finish_link_fingerprint(fingerprint),
        }
    }

    /// Fingerprints exactly `length` bytes from an already opened archive.
    /// A short read or trailing growth byte is rejected so callers can avoid a
    /// whole-archive allocation without weakening link-result identity.
    pub fn from_reader(
        package: impl Into<String>,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        reader: &mut impl Read,
        length: u64,
    ) -> io::Result<Self> {
        let mut fingerprint = QueryFingerprintBuilder::new(STATIC_ARCHIVE_LINK_INPUT_DOMAIN);
        let mut writer = fingerprint.bytes_writer(length);
        stream_fingerprint_bytes(reader, &mut writer, length)?;
        writer.finish()?;
        Ok(Self {
            package: package.into(),
            name: name.into(),
            path: path.into(),
            fingerprint: finish_link_fingerprint(fingerprint),
        })
    }

    /// Returns the physical archive path used in linker invocation.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn stream_fingerprint_bytes(
    reader: &mut impl Read,
    writer: &mut nia_query::QueryFingerprintBytesWriter<'_>,
    length: u64,
) -> io::Result<()> {
    let mut buffer = [0; 64 * 1024];
    let mut remaining = length;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        reader.read_exact(&mut buffer[..chunk_len])?;
        writer.write_chunk(&buffer[..chunk_len])?;
        remaining -= chunk_len as u64;
    }
    let mut trailing = [0; 1];
    if reader.read(&mut trailing)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew while it was fingerprinted",
        ));
    }
    Ok(())
}

fn write_fingerprint_file(
    fingerprint: &mut QueryFingerprintBuilder,
    path: &Path,
) -> io::Result<()> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let mut writer = fingerprint.bytes_writer(length);
    stream_fingerprint_bytes(&mut file, &mut writer, length)?;
    writer.finish()
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            linker: ExecutableLinker::native(),
            target: LinkTarget::host(),
            entry: Some("_start".to_string()),
            mode: LinkMode::Static,
            dynamic_linker: DynamicLinker::None,
            sysroot: None,
            library_paths: Vec::new(),
            rpaths: Vec::new(),
            libraries: Vec::new(),
            static_archives: Vec::new(),
            raw_args: Vec::new(),
        }
    }
}

impl LinkOptions {
    /// Computes complete executable-link identity when every external input is tracked.
    ///
    /// Returns `None` when a sysroot, native library, raw argument, unreadable
    /// linker, or unresolvable linker path prevents complete identity. Callers
    /// must execute the link without persistent result reuse in that case.
    pub fn result_fingerprint<T>(
        &self,
        inputs: &IncrementalLinkInputs<T>,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<Option<LinkResultFingerprintSet>, LinkerConfigError> {
        let Some(environment) = self.result_environment_fingerprint(toolchain_identity)? else {
            return Ok(None);
        };
        let mut cache_key = QueryFingerprintBuilder::new(LINK_RESULT_CACHE_KEY_DOMAIN);
        cache_key.write_u64(inputs.len() as u64);
        let mut input_component = QueryFingerprintBuilder::new(LINK_RESULT_INPUTS_DOMAIN);
        input_component.write_u64(inputs.len() as u64);
        for input in inputs.as_slice() {
            write_codegen_unit_key(&mut cache_key, &input.key);
            write_codegen_unit_key(&mut input_component, &input.key);
            for part in input.fingerprint.parts() {
                input_component.write_u64(part);
            }
        }
        cache_key.write_u64(self.static_archives.len() as u64);
        input_component.write_u64(self.static_archives.len() as u64);
        for archive in &self.static_archives {
            cache_key.write_str(&archive.package);
            cache_key.write_str(&archive.name);
            input_component.write_str(&archive.package);
            input_component.write_str(&archive.name);
            for part in archive.fingerprint.parts() {
                input_component.write_u64(part);
            }
        }

        Ok(Some(LinkResultFingerprintSet::new(
            LinkResultCacheKey::from_parts(cache_key.finish().parts()),
            LinkResultFingerprintComponents {
                inputs: finish_link_fingerprint(input_component),
                toolchain: environment.toolchain,
                target: environment.target,
                linker: environment.linker,
                options: environment.options,
            },
        )))
    }

    /// Tests whether cached non-input components still match the current environment.
    pub fn matches_result_environment(
        &self,
        expected: LinkResultFingerprintComponents,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<bool, LinkerConfigError> {
        let Some(current) = self.result_environment_fingerprint(toolchain_identity)? else {
            return Ok(false);
        };
        Ok(current.toolchain == expected.toolchain
            && current.target == expected.target
            && current.linker == expected.linker
            && current.options == expected.options)
    }

    /// Computes non-input identity when the environment is fully observable.
    ///
    /// Returns `None` under the same conservative conditions as
    /// [`Self::result_fingerprint`].
    pub fn result_environment_fingerprint(
        &self,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<Option<LinkResultEnvironmentFingerprint>, LinkerConfigError> {
        if self.sysroot.is_some() || !self.libraries.is_empty() || !self.raw_args.is_empty() {
            return Ok(None);
        }
        let linker = self.linker.resolve()?;
        let Some(linker_path) = resolved_linker_path(&linker.program) else {
            return Ok(None);
        };
        let mut toolchain = QueryFingerprintBuilder::new(LINK_RESULT_TOOLCHAIN_DOMAIN);
        for part in toolchain_identity.parts() {
            toolchain.write_u64(part);
        }
        let mut target = QueryFingerprintBuilder::new(LINK_RESULT_TARGET_DOMAIN);
        target.write_str(&self.target.arch);
        target.write_str(&self.target.os);
        target.write_str(&self.target.abi);
        if self.mode == LinkMode::Dynamic && self.dynamic_linker == DynamicLinker::Auto {
            write_optional_string(
                &mut target,
                dynamic_linker_for_target(&self.target)?.as_deref(),
            );
        }
        write_strings(&mut target, &self.default_library_paths_for_linker(&linker));

        let mut linker_component = QueryFingerprintBuilder::new(LINK_RESULT_LINKER_DOMAIN);
        linker_component.write_str(&linker_path.to_string_lossy());
        if write_fingerprint_file(&mut linker_component, &linker_path).is_err() {
            return Ok(None);
        }
        linker_component.write_u8(linker_flavor_tag(linker.flavor));

        let mut options = QueryFingerprintBuilder::new(LINK_RESULT_OPTIONS_DOMAIN);
        options.write_str(nia_compat::COMPILER_VERSION);
        write_optional_string(&mut options, self.entry.as_deref());
        options.write_u8(link_mode_tag(self.mode));
        write_dynamic_linker(&mut options, &self.dynamic_linker);
        write_strings(&mut options, &self.library_paths);
        write_strings(&mut options, &self.rpaths);

        Ok(Some(LinkResultEnvironmentFingerprint {
            toolchain: finish_link_fingerprint(toolchain),
            target: finish_link_fingerprint(target),
            linker: finish_link_fingerprint(linker_component),
            options: finish_link_fingerprint(options),
        }))
    }

    /// Replaces opaque linker arguments, which disable result caching.
    pub fn with_raw_args(mut self, args: Vec<String>) -> Self {
        self.raw_args = args;
        self
    }

    /// Overrides executable-linker selection.
    pub fn with_linker(mut self, linker: ExecutableLinker) -> Self {
        self.linker = linker;
        self
    }

    /// Overrides the artifact target.
    pub fn with_target(mut self, target: LinkTarget) -> Self {
        self.target = target;
        self
    }

    /// Overrides dynamic loader selection.
    pub fn with_dynamic_linker(mut self, dynamic_linker: DynamicLinker) -> Self {
        self.dynamic_linker = dynamic_linker;
        self
    }

    /// Selects dynamic mode and defaults an absent loader to automatic selection.
    pub fn with_dynamic_mode(mut self) -> Self {
        self.mode = LinkMode::Dynamic;
        if self.dynamic_linker == DynamicLinker::None {
            self.dynamic_linker = DynamicLinker::Auto;
        }
        self
    }

    /// Appends a native library search path.
    pub fn add_library_path(mut self, path: impl Into<String>) -> Self {
        self.library_paths.push(path.into());
        self
    }

    /// Appends a runtime library search path.
    pub fn add_rpath(mut self, path: impl Into<String>) -> Self {
        self.rpaths.push(path.into());
        self
    }

    /// Appends a native library following the enclosing link mode.
    pub fn add_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::default(library));
        self
    }

    /// Appends a native library forced to static search.
    pub fn add_static_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::static_(library));
        self
    }

    /// Appends a native library forced to dynamic search.
    pub fn add_dynamic_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::dynamic(library));
        self
    }

    /// Replaces typed static archive inputs.
    pub fn with_static_archives(mut self, archives: Vec<StaticArchiveLinkInput>) -> Self {
        self.static_archives = archives;
        self
    }

    /// Builds a resolved linker process invocation for ordered typed inputs.
    pub fn invocation(
        &self,
        inputs: &IncrementalLinkInputs<PathBuf>,
        output: PathBuf,
    ) -> Result<LinkerInvocation, LinkerConfigError> {
        let linker = self.linker.resolve()?;
        match self.linker.flavor {
            LinkerFlavor::Gnu | LinkerFlavor::Lld => {
                self.gnu_like_invocation(&linker, inputs, output)
            }
            LinkerFlavor::SelfHostedElf => {
                Err(LinkerConfigError::UnsupportedFlavor(self.linker.flavor))
            }
        }
    }

    fn gnu_like_invocation(
        &self,
        linker: &ResolvedLinker,
        inputs: &IncrementalLinkInputs<PathBuf>,
        output: PathBuf,
    ) -> Result<LinkerInvocation, LinkerConfigError> {
        let mut args = Vec::new();
        if let Some(sysroot) = &self.sysroot {
            args.push(format!("--sysroot={sysroot}"));
        }
        if let Some(entry) = &self.entry {
            args.push("-e".to_string());
            args.push(entry.clone());
        }
        if let Some(emulation) = gnu_emulation_for_target(&self.target) {
            args.push("-m".to_string());
            args.push(emulation.to_string());
        }
        args.extend(
            inputs
                .as_slice()
                .iter()
                .map(|input| input.object.to_string_lossy().into_owned()),
        );
        args.extend(
            self.static_archives
                .iter()
                .map(|archive| archive.path.to_string_lossy().into_owned()),
        );
        match self.mode {
            LinkMode::Static => {
                args.push("-static".to_string());
            }
            LinkMode::Dynamic => {}
        }
        for path in self.default_library_paths_for_linker(linker) {
            args.push("-L".to_string());
            args.push(path);
        }
        for path in &self.library_paths {
            args.push("-L".to_string());
            args.push(path.clone());
        }
        for rpath in &self.rpaths {
            args.push("-rpath".to_string());
            args.push(rpath.clone());
        }
        self.push_gnu_like_libraries(&mut args);
        match self.mode {
            LinkMode::Static => {}
            LinkMode::Dynamic => match &self.dynamic_linker {
                DynamicLinker::Auto => {
                    if let Some(path) = dynamic_linker_for_target(&self.target)? {
                        args.push("--dynamic-linker".to_string());
                        args.push(path);
                    } else {
                        args.push("--no-dynamic-linker".to_string());
                    }
                }
                DynamicLinker::None => {
                    args.push("--no-dynamic-linker".to_string());
                }
                DynamicLinker::Path(path) => {
                    args.push("--dynamic-linker".to_string());
                    args.push(path.clone());
                }
            },
        }
        args.extend(self.raw_args.iter().cloned());
        args.push("-o".to_string());
        args.push(output.to_string_lossy().into_owned());
        Ok(LinkerInvocation {
            program: linker.program.clone(),
            args,
        })
    }

    fn push_gnu_like_libraries(&self, args: &mut Vec<String>) {
        let mut current_mode = LibraryLinkMode::Default;
        for library in &self.libraries {
            if library.mode != current_mode {
                match library.mode {
                    LibraryLinkMode::Default => {
                        args.push(
                            match self.mode {
                                LinkMode::Static => "-Bstatic",
                                LinkMode::Dynamic => "-Bdynamic",
                            }
                            .to_string(),
                        );
                    }
                    LibraryLinkMode::Static => args.push("-Bstatic".to_string()),
                    LibraryLinkMode::Dynamic => args.push("-Bdynamic".to_string()),
                }
                current_mode = library.mode;
            }
            args.push("-l".to_string());
            args.push(library.name.clone());
        }
        if current_mode != LibraryLinkMode::Default {
            let default_flag = match self.mode {
                LinkMode::Static => "-Bstatic",
                LinkMode::Dynamic => "-Bdynamic",
            };
            let current_flag = match current_mode {
                LibraryLinkMode::Default => default_flag,
                LibraryLinkMode::Static => "-Bstatic",
                LibraryLinkMode::Dynamic => "-Bdynamic",
            };
            if current_flag != default_flag {
                args.push(default_flag.to_string());
            }
        }
    }

    fn default_library_paths_for_linker(&self, linker: &ResolvedLinker) -> Vec<String> {
        if linker.flavor != LinkerFlavor::Lld || self.sysroot.is_some() || !self.target.is_host() {
            return Vec::new();
        }
        native_linux_library_paths()
    }
}

fn gnu_emulation_for_target(target: &LinkTarget) -> Option<&'static str> {
    (target.os == "linux" && matches!(target.arch.as_str(), "x86" | "i386" | "i586" | "i686"))
        .then_some("elf_i386")
}

fn resolved_linker_path(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    let resolved = if path.components().count() > 1 {
        path.to_path_buf()
    } else {
        PathBuf::from(find_program_on_path(program)?)
    };
    resolved.canonicalize().ok()
}

fn finish_link_fingerprint(builder: QueryFingerprintBuilder) -> LinkResultFingerprint {
    LinkResultFingerprint::from_parts(builder.finish().parts())
}

fn finish_archive_fingerprint(builder: QueryFingerprintBuilder) -> ArchiveFingerprint {
    ArchiveFingerprint::from_parts(builder.finish().parts())
}

fn write_codegen_unit_key(builder: &mut QueryFingerprintBuilder, key: &CodegenUnitKey) {
    match key {
        CodegenUnitKey::SourceModule {
            source_identity,
            ordinal,
        } => {
            builder.write_u8(0);
            builder.write_str(source_identity.normalized_path());
            builder.write_u64(u64::from(*ordinal));
        }
        CodegenUnitKey::CompilerBuiltins => builder.write_u8(1),
    }
}

fn write_optional_string(builder: &mut QueryFingerprintBuilder, value: Option<&str>) {
    match value {
        Some(value) => {
            builder.write_u8(1);
            builder.write_str(value);
        }
        None => builder.write_u8(0),
    }
}

fn write_strings(builder: &mut QueryFingerprintBuilder, values: &[String]) {
    builder.write_u64(values.len() as u64);
    for value in values {
        builder.write_str(value);
    }
}

fn write_dynamic_linker(builder: &mut QueryFingerprintBuilder, linker: &DynamicLinker) {
    match linker {
        DynamicLinker::Auto => builder.write_u8(0),
        DynamicLinker::None => builder.write_u8(1),
        DynamicLinker::Path(path) => {
            builder.write_u8(2);
            builder.write_str(path);
        }
    }
}

const fn linker_flavor_tag(flavor: LinkerFlavor) -> u8 {
    match flavor {
        LinkerFlavor::Gnu => 0,
        LinkerFlavor::Lld => 1,
        LinkerFlavor::SelfHostedElf => 2,
    }
}

const fn link_mode_tag(mode: LinkMode) -> u8 {
    match mode {
        LinkMode::Static => 0,
        LinkMode::Dynamic => 1,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLinker {
    program: String,
    flavor: LinkerFlavor,
}

impl ExecutableLinker {
    fn resolve(&self) -> Result<ResolvedLinker, LinkerConfigError> {
        match self.flavor {
            LinkerFlavor::Gnu => Ok(ResolvedLinker {
                program: if self.program.is_empty() {
                    "ld".to_string()
                } else {
                    self.program.clone()
                },
                flavor: self.flavor,
            }),
            LinkerFlavor::Lld => Ok(ResolvedLinker {
                program: resolve_lld_program(&self.program)?,
                flavor: self.flavor,
            }),
            LinkerFlavor::SelfHostedElf => Err(LinkerConfigError::UnsupportedFlavor(self.flavor)),
        }
    }
}

/// Fully resolved executable-link process invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkerInvocation {
    /// Linker executable path.
    pub program: String,
    /// Ordered command-line arguments.
    pub args: Vec<String>,
}

/// Failure while resolving a linker, archive tool, or target runtime metadata.
#[derive(Debug)]
pub enum LinkerConfigError {
    /// An explicitly selected archive tool was not found.
    ArchiveToolNotFound {
        /// Requested program name/path.
        program: String,
    },
    /// A filesystem or executable inspection operation failed.
    Io {
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying I/O error.
        error: io::Error,
    },
    /// A file expected to contain a supported ELF image was malformed.
    InvalidElf {
        /// ELF path.
        path: PathBuf,
    },
    /// The requested linker flavor could not be discovered.
    LinkerNotFound {
        /// Requested command-line flavor.
        flavor: LinkerFlavor,
        /// Program searched for.
        program: String,
    },
    /// The flavor is reserved but has no invocation implementation.
    UnsupportedFlavor(LinkerFlavor),
}

impl std::fmt::Display for LinkerConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArchiveToolNotFound { program } => {
                write!(f, "archive tool `{program}` was not found")
            }
            Self::Io { path, error } => {
                write!(f, "failed to inspect `{}`: {error}", path.display())
            }
            Self::InvalidElf { path } => write!(f, "`{}` is not a valid ELF file", path.display()),
            Self::LinkerNotFound { flavor, program } => {
                write!(
                    f,
                    "linker `{program}` for flavor `{flavor:?}` was not found"
                )
            }
            Self::UnsupportedFlavor(flavor) => {
                write!(f, "linker flavor `{flavor:?}` is not implemented")
            }
        }
    }
}

impl std::error::Error for LinkerConfigError {}

fn resolve_lld_program(program: &str) -> Result<String, LinkerConfigError> {
    if !program.is_empty() {
        return Ok(program.to_string());
    }
    if let Ok(program) = env::var("NIA_LLD")
        && !program.is_empty()
    {
        return Ok(program);
    }
    find_program_on_path("ld.lld").ok_or_else(|| LinkerConfigError::LinkerNotFound {
        flavor: LinkerFlavor::Lld,
        program: "ld.lld".to_string(),
    })
}

fn find_program_on_path(program: &str) -> Option<String> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return is_executable_file(program_path).then(|| program.to_string());
    }
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn native_linux_library_paths() -> Vec<String> {
    if env::consts::OS != "linux" {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for path in [
        "/usr/local/lib64",
        "/usr/lib64",
        "/lib64",
        "/usr/local/lib",
        "/usr/lib",
        "/lib",
    ] {
        insert_existing_library_path(&mut paths, path);
    }
    read_ld_so_conf(&mut paths, Path::new("/etc/ld.so.conf"), 0);
    paths
}

fn insert_existing_library_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && Path::new(path).is_dir() {
        let path = path.to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
}

fn read_ld_so_conf(paths: &mut Vec<String>, path: &Path, depth: usize) {
    let mut budget = LdSoConfReadBudget {
        remaining_bytes: MAX_LD_SO_CONF_TOTAL_BYTES,
        remaining_files: MAX_LD_SO_CONF_FILES,
        visited: HashSet::new(),
    };
    read_ld_so_conf_bounded(paths, path, depth, &mut budget);
}

struct LdSoConfReadBudget {
    remaining_bytes: usize,
    remaining_files: usize,
    visited: HashSet<PathBuf>,
}

fn read_ld_so_conf_bounded(
    paths: &mut Vec<String>,
    path: &Path,
    depth: usize,
    budget: &mut LdSoConfReadBudget,
) {
    if depth > 8 {
        return;
    }
    let Ok(canonical_path) = fs::canonicalize(path) else {
        return;
    };
    if budget.remaining_files == 0 || !budget.visited.insert(canonical_path.clone()) {
        return;
    }
    let Ok(mut file) = fs::File::open(&canonical_path) else {
        return;
    };
    let Ok(file_len) = file.metadata().map(|metadata| metadata.len()) else {
        return;
    };
    let allowed = budget.remaining_bytes.min(MAX_LD_SO_CONF_FILE_BYTES);
    if file_len > allowed as u64 {
        return;
    }
    let mut encoded = Vec::with_capacity(usize::try_from(file_len).unwrap_or(allowed));
    if file
        .by_ref()
        .take((allowed + 1) as u64)
        .read_to_end(&mut encoded)
        .is_err()
        || encoded.len() > allowed
    {
        return;
    }
    let Ok(contents) = String::from_utf8(encoded) else {
        return;
    };
    budget.remaining_bytes -= contents.len();
    budget.remaining_files -= 1;
    let base = path.parent().unwrap_or_else(|| Path::new("/"));
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line.strip_prefix("include ") {
            read_ld_so_conf_include(paths, base, pattern.trim(), depth + 1, budget);
        } else {
            insert_existing_library_path(paths, line);
        }
    }
}

fn read_ld_so_conf_include(
    paths: &mut Vec<String>,
    base: &Path,
    pattern: &str,
    depth: usize,
    budget: &mut LdSoConfReadBudget,
) {
    let pattern_path = Path::new(pattern);
    let pattern_path = if pattern_path.is_absolute() {
        pattern_path.to_path_buf()
    } else {
        base.join(pattern_path)
    };
    let Some(parent) = pattern_path.parent() else {
        return;
    };
    let Some(file_pattern) = pattern_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut matching = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if glob_file_name_matches(file_pattern, file_name) {
            if matching.len() == MAX_LD_SO_CONF_INCLUDE_ENTRIES {
                return;
            }
            matching.push(path);
        }
    }
    matching.sort_unstable();
    for path in matching {
        read_ld_so_conf_bounded(paths, &path, depth, budget);
    }
}

fn glob_file_name_matches(pattern: &str, file_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let Some(star) = pattern.find('*') else {
        return pattern == file_name;
    };
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    file_name.starts_with(prefix) && file_name.ends_with(suffix)
}

/// Reads the host executable's ELF interpreter when supported by the host OS.
pub fn native_dynamic_linker() -> Result<Option<String>, LinkerConfigError> {
    #[cfg(target_os = "linux")]
    {
        let path = PathBuf::from("/usr/bin/env");
        match elf_interpreter(&path) {
            Ok(Some(path)) => Ok(Some(path)),
            Ok(None) | Err(LinkerConfigError::InvalidElf { .. }) => Ok(standard_dynamic_linker()),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// Returns the standard dynamic linker path for the process host target.
pub fn standard_dynamic_linker() -> Option<String> {
    standard_dynamic_linker_for(&LinkTarget::host())
}

/// Returns a known standard dynamic linker path for a target architecture/ABI.
pub fn standard_dynamic_linker_for(target: &LinkTarget) -> Option<String> {
    if target.os != "linux" {
        return None;
    }
    if is_musl_abi(&target.abi) {
        return musl_dynamic_linker(target);
    }
    if is_gnu_abi(&target.abi) {
        return gnu_dynamic_linker(target);
    }
    None
}

fn dynamic_linker_for_target(target: &LinkTarget) -> Result<Option<String>, LinkerConfigError> {
    if target.is_host() {
        native_dynamic_linker()
    } else {
        Ok(standard_dynamic_linker_for(target))
    }
}

fn gnu_dynamic_linker(target: &LinkTarget) -> Option<String> {
    match (target.arch.as_str(), target.abi.as_str()) {
        ("x86_64", "gnu") => Some("/lib64/ld-linux-x86-64.so.2".to_string()),
        ("x86_64", "gnux32") => Some("/libx32/ld-linux-x32.so.2".to_string()),
        ("x86", "gnu") | ("i386", "gnu") | ("i586", "gnu") | ("i686", "gnu") => {
            Some("/lib/ld-linux.so.2".to_string())
        }
        ("aarch64", "gnu") => Some("/lib/ld-linux-aarch64.so.1".to_string()),
        ("aarch64_be", "gnu") => Some("/lib/ld-linux-aarch64_be.so.1".to_string()),
        ("arm", "gnueabi") | ("armeb", "gnueabi") | ("thumb", "gnueabi") => {
            Some("/lib/ld-linux.so.3".to_string())
        }
        ("arm", "gnueabihf") | ("armeb", "gnueabihf") | ("thumb", "gnueabihf") => {
            Some("/lib/ld-linux-armhf.so.3".to_string())
        }
        ("riscv64", "gnu") => Some("/lib/ld-linux-riscv64-lp64d.so.1".to_string()),
        ("riscv32", "gnu") => Some("/lib/ld-linux-riscv32-ilp32d.so.1".to_string()),
        ("powerpc64", "gnu") | ("powerpc64le", "gnu") => Some("/lib64/ld64.so.2".to_string()),
        ("s390x", "gnu") => Some("/lib/ld64.so.1".to_string()),
        ("mips", "gnueabi")
        | ("mipsel", "gnueabi")
        | ("mips", "gnueabihf")
        | ("mipsel", "gnueabihf") => Some("/lib/ld.so.1".to_string()),
        ("mips64", "gnuabi64") | ("mips64el", "gnuabi64") => Some("/lib64/ld.so.1".to_string()),
        ("mips64", "gnuabin32") | ("mips64el", "gnuabin32") => Some("/lib32/ld.so.1".to_string()),
        ("loongarch64", "gnu") => Some("/lib64/ld-linux-loongarch-lp64d.so.1".to_string()),
        ("loongarch64", "gnuf32") => Some("/lib64/ld-linux-loongarch-lp64f.so.1".to_string()),
        ("loongarch64", "gnusf") => Some("/lib64/ld-linux-loongarch-lp64s.so.1".to_string()),
        ("sparc64", "gnu") => Some("/lib64/ld-linux.so.2".to_string()),
        ("sparc", "gnu") | ("alpha", "gnu") => Some("/lib/ld-linux.so.2".to_string()),
        ("hppa", "gnu") | ("m68k", "gnu") | ("microblaze", "gnu") | ("microblazeel", "gnu") => {
            Some("/lib/ld.so.1".to_string())
        }
        _ => None,
    }
}

fn musl_dynamic_linker(target: &LinkTarget) -> Option<String> {
    match (target.arch.as_str(), target.abi.as_str()) {
        ("x86_64", "musl") => Some("/lib/ld-musl-x86_64.so.1".to_string()),
        ("x86_64", "muslx32") => Some("/lib/ld-musl-x32.so.1".to_string()),
        ("x86", "musl") | ("i386", "musl") | ("i586", "musl") | ("i686", "musl") => {
            Some("/lib/ld-musl-i386.so.1".to_string())
        }
        ("aarch64", "musl") => Some("/lib/ld-musl-aarch64.so.1".to_string()),
        ("aarch64_be", "musl") => Some("/lib/ld-musl-aarch64_be.so.1".to_string()),
        ("arm", "musleabi") | ("thumb", "musleabi") => Some("/lib/ld-musl-arm.so.1".to_string()),
        ("arm", "musleabihf") | ("thumb", "musleabihf") => {
            Some("/lib/ld-musl-armhf.so.1".to_string())
        }
        ("armeb", "musleabi") | ("thumbeb", "musleabi") => {
            Some("/lib/ld-musl-armeb.so.1".to_string())
        }
        ("armeb", "musleabihf") | ("thumbeb", "musleabihf") => {
            Some("/lib/ld-musl-armebhf.so.1".to_string())
        }
        ("riscv64", "musl") => Some("/lib/ld-musl-riscv64.so.1".to_string()),
        ("riscv32", "musl") => Some("/lib/ld-musl-riscv32.so.1".to_string()),
        ("powerpc64", "musl") => Some("/lib/ld-musl-powerpc64.so.1".to_string()),
        ("powerpc64le", "musl") => Some("/lib/ld-musl-powerpc64le.so.1".to_string()),
        ("powerpc", "musleabi") => Some("/lib/ld-musl-powerpc-sf.so.1".to_string()),
        ("powerpc", "musleabihf") => Some("/lib/ld-musl-powerpc.so.1".to_string()),
        ("mips", "musleabi") => Some("/lib/ld-musl-mips-sf.so.1".to_string()),
        ("mips", "musleabihf") => Some("/lib/ld-musl-mips.so.1".to_string()),
        ("mipsel", "musleabi") => Some("/lib/ld-musl-mipsel-sf.so.1".to_string()),
        ("mipsel", "musleabihf") => Some("/lib/ld-musl-mipsel.so.1".to_string()),
        ("mips64", "muslabi64") => Some("/lib/ld-musl-mips64.so.1".to_string()),
        ("mips64el", "muslabi64") => Some("/lib/ld-musl-mips64el.so.1".to_string()),
        ("mips64", "muslabin32") => Some("/lib/ld-musl-mipsn32.so.1".to_string()),
        ("mips64el", "muslabin32") => Some("/lib/ld-musl-mipsn32el.so.1".to_string()),
        ("s390x", "musl") => Some("/lib/ld-musl-s390x.so.1".to_string()),
        ("loongarch64", "musl") => Some("/lib/ld-musl-loongarch64.so.1".to_string()),
        ("m68k", "musl") => Some("/lib/ld-musl-m68k.so.1".to_string()),
        ("microblaze", "musl") => Some("/lib/ld-musl-microblaze.so.1".to_string()),
        ("microblazeel", "musl") => Some("/lib/ld-musl-microblazeel.so.1".to_string()),
        _ => None,
    }
}

fn is_gnu_abi(abi: &str) -> bool {
    matches!(
        abi,
        "gnu" | "gnux32" | "gnueabi" | "gnueabihf" | "gnuabi64" | "gnuabin32" | "gnuf32" | "gnusf"
    )
}

fn is_musl_abi(abi: &str) -> bool {
    matches!(
        abi,
        "musl"
            | "muslx32"
            | "musleabi"
            | "musleabihf"
            | "muslabi64"
            | "muslabin32"
            | "muslf32"
            | "muslsf"
    )
}

fn default_host_abi() -> String {
    default_abi_for_os(env::consts::OS)
}

fn default_abi_for_os(os: &str) -> String {
    match os {
        "linux" => "gnu".to_string(),
        _ => String::new(),
    }
}

fn elf_interpreter(path: &Path) -> Result<Option<String>, LinkerConfigError> {
    const EI_CLASS: usize = 4;
    const EI_DATA: usize = 5;
    const ELFCLASS64: u8 = 2;
    const ELFDATA2LSB: u8 = 1;
    const PT_INTERP: u32 = 3;
    const ELF_HEADER_LEN: usize = 64;
    const PROGRAM_HEADER_LEN_64: u64 = 56;
    const MAX_INTERPRETER_BYTES: u64 = 4096;

    let mut file = fs::File::open(path).map_err(|error| LinkerConfigError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let file_len = file
        .metadata()
        .map_err(|error| LinkerConfigError::Io {
            path: path.to_path_buf(),
            error,
        })?
        .len();
    if file_len < ELF_HEADER_LEN as u64 {
        return Err(invalid_elf(path));
    }
    let mut header = [0; ELF_HEADER_LEN];
    read_elf_bytes(&mut file, path, 0, &mut header)?;
    if &header[0..4] != b"\x7fELF"
        || header[EI_CLASS] != ELFCLASS64
        || header[EI_DATA] != ELFDATA2LSB
    {
        return Err(invalid_elf(path));
    }

    let phoff = read_u64(&header, 32).ok_or_else(|| invalid_elf(path))?;
    let phentsize = u64::from(read_u16(&header, 54).ok_or_else(|| invalid_elf(path))?);
    let phnum = u64::from(read_u16(&header, 56).ok_or_else(|| invalid_elf(path))?);
    if phentsize < PROGRAM_HEADER_LEN_64 {
        return Err(invalid_elf(path));
    }
    let table_len = phentsize
        .checked_mul(phnum)
        .ok_or_else(|| invalid_elf(path))?;
    let table_end = phoff
        .checked_add(table_len)
        .ok_or_else(|| invalid_elf(path))?;
    if table_end > file_len {
        return Err(invalid_elf(path));
    }

    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        let mut program_header = [0; PROGRAM_HEADER_LEN_64 as usize];
        read_elf_bytes(&mut file, path, offset, &mut program_header)?;
        let p_type = read_u32(&program_header, 0).ok_or_else(|| invalid_elf(path))?;
        if p_type != PT_INTERP {
            continue;
        }
        let p_offset = read_u64(&program_header, 8).ok_or_else(|| invalid_elf(path))?;
        let p_filesz = read_u64(&program_header, 32).ok_or_else(|| invalid_elf(path))?;
        let interpreter_end = p_offset
            .checked_add(p_filesz)
            .ok_or_else(|| invalid_elf(path))?;
        if p_filesz > MAX_INTERPRETER_BYTES || interpreter_end > file_len {
            return Err(invalid_elf(path));
        }
        let interpreter_len = usize::try_from(p_filesz).map_err(|_| invalid_elf(path))?;
        let mut interpreter = vec![0; interpreter_len];
        read_elf_bytes(&mut file, path, p_offset, &mut interpreter)?;
        let nul = interpreter
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(interpreter.len());
        interpreter.truncate(nul);
        return String::from_utf8(interpreter)
            .map(Some)
            .map_err(|_| invalid_elf(path));
    }
    Ok(None)
}

fn invalid_elf(path: &Path) -> LinkerConfigError {
    LinkerConfigError::InvalidElf {
        path: path.to_path_buf(),
    }
}

fn read_elf_bytes(
    file: &mut fs::File,
    path: &Path,
    offset: u64,
    bytes: &mut [u8],
) -> Result<(), LinkerConfigError> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(bytes))
        .map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                invalid_elf(path)
            } else {
                LinkerConfigError::Io {
                    path: path.to_path_buf(),
                    error,
                }
            }
        })
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let array = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let array = bytes.get(offset..offset + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey, IncrementalLinkInput};
    use nia_source::SourceIdentity;
    use std::sync::Mutex;

    include!("tests/linker/test_support.rs");

    #[test]
    fn streamed_static_archive_identity_matches_bytes_and_rejects_length_changes() {
        let expected = StaticArchiveLinkInput::from_bytes("root", "support", "lib.a", b"archive");
        let streamed = StaticArchiveLinkInput::from_reader(
            "root",
            "support",
            "lib.a",
            &mut io::Cursor::new(b"archive"),
            7,
        )
        .expect("stream archive identity");
        assert_eq!(streamed, expected);

        let growth = StaticArchiveLinkInput::from_reader(
            "root",
            "support",
            "lib.a",
            &mut io::Cursor::new(b"archive!"),
            7,
        )
        .expect_err("archive growth must be rejected");
        assert_eq!(growth.kind(), io::ErrorKind::InvalidData);

        let truncation = StaticArchiveLinkInput::from_reader(
            "root",
            "support",
            "lib.a",
            &mut io::Cursor::new(b"archive"),
            8,
        )
        .expect_err("archive truncation must be rejected");
        assert_eq!(truncation.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn streamed_file_fingerprint_matches_bytes_across_buffer_boundaries() {
        let bytes = (0..(64 * 1024 * 3 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let path = fingerprint_linker("streamed-tool-fingerprint", &bytes);
        let mut expected = QueryFingerprintBuilder::new(ARCHIVE_TOOL_DOMAIN);
        expected.write_bytes(&bytes);
        let mut streamed = QueryFingerprintBuilder::new(ARCHIVE_TOOL_DOMAIN);

        write_fingerprint_file(&mut streamed, &path).expect("stream tool fingerprint");

        assert_eq!(streamed.finish(), expected.finish());
    }

    #[path = "linker/fingerprint_contracts.rs"]
    mod fingerprint_contracts;

    #[path = "linker/gnu_invocation.rs"]
    mod gnu_invocation;

    #[path = "linker/lld_resolution.rs"]
    mod lld_resolution;

    #[path = "linker/dynamic_linker_contracts.rs"]
    mod dynamic_linker_contracts;

    #[path = "linker/archive_contracts.rs"]
    mod archive_contracts;
}
