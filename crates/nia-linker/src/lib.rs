// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use nia_backend_ir::{CodegenUnitKey, IncrementalLinkInputs};
use nia_query::QueryFingerprintBuilder;
use nia_target_config::TargetConfig;

const LINK_RESULT_FINGERPRINT_DOMAIN: &str = "nia.link-result-components.v2";
const ARCHIVE_RESULT_FINGERPRINT_DOMAIN: &str = "nia.archive-result-components.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkResultFingerprint([u64; 2]);

impl LinkResultFingerprint {
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkResultCacheKey([u64; 2]);

impl LinkResultCacheKey {
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultFingerprintComponents {
    pub inputs: LinkResultFingerprint,
    pub toolchain: LinkResultFingerprint,
    pub target: LinkResultFingerprint,
    pub linker: LinkResultFingerprint,
    pub options: LinkResultFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultFingerprintSet {
    pub cache_key: LinkResultCacheKey,
    pub fingerprint: LinkResultFingerprint,
    pub components: LinkResultFingerprintComponents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultEnvironmentFingerprint {
    pub toolchain: LinkResultFingerprint,
    pub target: LinkResultFingerprint,
    pub linker: LinkResultFingerprint,
    pub options: LinkResultFingerprint,
}

impl LinkResultFingerprintSet {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResultInvalidation {
    pub inputs: bool,
    pub toolchain: bool,
    pub target: bool,
    pub linker: bool,
    pub options: bool,
}

impl LinkResultInvalidation {
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

    pub fn count(self) -> u32 {
        u32::from(self.inputs)
            + u32::from(self.toolchain)
            + u32::from(self.target)
            + u32::from(self.linker)
            + u32::from(self.options)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    Gnu,
    Lld,
    SelfHostedElf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    Static,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicLinker {
    Auto,
    None,
    Path(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryLinkMode {
    Default,
    Static,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLibrary {
    pub name: String,
    pub mode: LibraryLinkMode,
}

impl NativeLibrary {
    pub fn default(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Default,
        }
    }

    pub fn static_(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Static,
        }
    }

    pub fn dynamic(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: LibraryLinkMode::Dynamic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableLinker {
    pub program: String,
    pub flavor: LinkerFlavor,
}

impl ExecutableLinker {
    pub fn native() -> Self {
        if let Ok(program) = env::var("NIA_LINKER")
            && !program.is_empty()
        {
            return Self::with_program(program);
        }
        Self::with_program("ld")
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            flavor: LinkerFlavor::Gnu,
        }
    }

    pub fn with_program_and_flavor(program: impl Into<String>, flavor: LinkerFlavor) -> Self {
        Self {
            program: program.into(),
            flavor,
        }
    }

    pub fn lld() -> Self {
        Self {
            program: String::new(),
            flavor: LinkerFlavor::Lld,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTool {
    pub program: String,
}

impl ArchiveTool {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchiveFingerprint([u64; 2]);

impl ArchiveFingerprint {
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchiveCacheKey([u64; 2]);

impl ArchiveCacheKey {
    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(parts)
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFingerprintComponents {
    pub inputs: ArchiveFingerprint,
    pub toolchain: ArchiveFingerprint,
    pub target: ArchiveFingerprint,
    pub tool: ArchiveFingerprint,
    pub options: ArchiveFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFingerprintSet {
    pub cache_key: ArchiveCacheKey,
    pub fingerprint: ArchiveFingerprint,
    pub components: ArchiveFingerprintComponents,
}

impl ArchiveFingerprintSet {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveInvalidation {
    pub inputs: bool,
    pub toolchain: bool,
    pub target: bool,
    pub tool: bool,
    pub options: bool,
}

impl ArchiveInvalidation {
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

    pub fn count(self) -> u32 {
        u32::from(self.inputs)
            + u32::from(self.toolchain)
            + u32::from(self.target)
            + u32::from(self.tool)
            + u32::from(self.options)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveEnvironmentFingerprint {
    pub toolchain: ArchiveFingerprint,
    pub target: ArchiveFingerprint,
    pub tool: ArchiveFingerprint,
    pub options: ArchiveFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOptions {
    pub target: LinkTarget,
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
    pub fn with_target(mut self, target: LinkTarget) -> Self {
        self.target = target;
        self
    }

    pub fn with_tool(mut self, tool: ArchiveTool) -> Self {
        self.tool = tool;
        self
    }

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
        let tool_bytes = fs::read(&program_path).map_err(|error| LinkerConfigError::Io {
            path: program_path.clone(),
            error,
        })?;
        let mut toolchain = QueryFingerprintBuilder::new("nia.archive-toolchain.v1");
        for part in toolchain_identity.parts() {
            toolchain.write_u64(part);
        }
        let mut target = QueryFingerprintBuilder::new("nia.archive-target.v1");
        target.write_str(&self.target.arch);
        target.write_str(&self.target.os);
        target.write_str(&self.target.abi);
        let mut tool = QueryFingerprintBuilder::new("nia.archive-tool.v1");
        tool.write_str(&program_path.to_string_lossy());
        tool.write_bytes(&tool_bytes);
        let mut options = QueryFingerprintBuilder::new("nia.archive-options.v1");
        options.write_str(env!("CARGO_PKG_VERSION"));
        options.write_str("rcsD");
        Ok(ArchiveEnvironmentFingerprint {
            toolchain: finish_archive_fingerprint(toolchain),
            target: finish_archive_fingerprint(target),
            tool: finish_archive_fingerprint(tool),
            options: finish_archive_fingerprint(options),
        })
    }

    pub fn result_fingerprint<T>(
        &self,
        inputs: &IncrementalLinkInputs<T>,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<ArchiveFingerprintSet, LinkerConfigError> {
        let environment = self.environment_fingerprint(toolchain_identity)?;
        let mut cache_key = QueryFingerprintBuilder::new("nia.archive-result-cache-key.v1");
        cache_key.write_u64(inputs.len() as u64);
        let mut input_component = QueryFingerprintBuilder::new("nia.archive-result-inputs.v1");
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInvocation {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    pub arch: String,
    pub os: String,
    pub abi: String,
}

impl LinkTarget {
    pub fn host() -> Self {
        Self {
            arch: env::consts::ARCH.to_string(),
            os: env::consts::OS.to_string(),
            abi: default_host_abi(),
        }
    }

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

    pub fn is_host(&self) -> bool {
        let host = Self::host();
        self.arch == host.arch && self.os == host.os && self.abi == host.abi
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkOptions {
    pub target: LinkTarget,
    pub linker: ExecutableLinker,
    pub entry: Option<String>,
    pub mode: LinkMode,
    pub dynamic_linker: DynamicLinker,
    pub sysroot: Option<String>,
    pub library_paths: Vec<String>,
    pub rpaths: Vec<String>,
    pub libraries: Vec<NativeLibrary>,
    pub raw_args: Vec<String>,
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
            raw_args: Vec::new(),
        }
    }
}

impl LinkOptions {
    pub fn result_fingerprint<T>(
        &self,
        inputs: &IncrementalLinkInputs<T>,
        toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    ) -> Result<Option<LinkResultFingerprintSet>, LinkerConfigError> {
        let Some(environment) = self.result_environment_fingerprint(toolchain_identity)? else {
            return Ok(None);
        };
        let mut cache_key = QueryFingerprintBuilder::new("nia.link-result-cache-key.v2");
        cache_key.write_u64(inputs.len() as u64);
        let mut input_component = QueryFingerprintBuilder::new("nia.link-result-inputs.v2");
        input_component.write_u64(inputs.len() as u64);
        for input in inputs.as_slice() {
            write_codegen_unit_key(&mut cache_key, &input.key);
            write_codegen_unit_key(&mut input_component, &input.key);
            for part in input.fingerprint.parts() {
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
        let linker_bytes = match fs::read(&linker_path) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        let mut toolchain = QueryFingerprintBuilder::new("nia.link-result-toolchain.v1");
        for part in toolchain_identity.parts() {
            toolchain.write_u64(part);
        }
        let mut target = QueryFingerprintBuilder::new("nia.link-result-target.v2");
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

        let mut linker_component = QueryFingerprintBuilder::new("nia.link-result-linker.v2");
        linker_component.write_str(&linker_path.to_string_lossy());
        linker_component.write_bytes(&linker_bytes);
        linker_component.write_u8(linker_flavor_tag(linker.flavor));

        let mut options = QueryFingerprintBuilder::new("nia.link-result-options.v2");
        options.write_str(env!("CARGO_PKG_VERSION"));
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

    pub fn with_raw_args(mut self, args: Vec<String>) -> Self {
        self.raw_args = args;
        self
    }

    pub fn with_linker(mut self, linker: ExecutableLinker) -> Self {
        self.linker = linker;
        self
    }

    pub fn with_target(mut self, target: LinkTarget) -> Self {
        self.target = target;
        self
    }

    pub fn with_dynamic_linker(mut self, dynamic_linker: DynamicLinker) -> Self {
        self.dynamic_linker = dynamic_linker;
        self
    }

    pub fn with_dynamic_mode(mut self) -> Self {
        self.mode = LinkMode::Dynamic;
        if self.dynamic_linker == DynamicLinker::None {
            self.dynamic_linker = DynamicLinker::Auto;
        }
        self
    }

    pub fn add_library_path(mut self, path: impl Into<String>) -> Self {
        self.library_paths.push(path.into());
        self
    }

    pub fn add_rpath(mut self, path: impl Into<String>) -> Self {
        self.rpaths.push(path.into());
        self
    }

    pub fn add_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::default(library));
        self
    }

    pub fn add_static_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::static_(library));
        self
    }

    pub fn add_dynamic_library(mut self, library: impl Into<String>) -> Self {
        self.libraries.push(NativeLibrary::dynamic(library));
        self
    }

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
        args.extend(
            inputs
                .as_slice()
                .iter()
                .map(|input| input.object.to_string_lossy().into_owned()),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkerInvocation {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug)]
pub enum LinkerConfigError {
    ArchiveToolNotFound {
        program: String,
    },
    Io {
        path: PathBuf,
        error: io::Error,
    },
    InvalidElf {
        path: PathBuf,
    },
    LinkerNotFound {
        flavor: LinkerFlavor,
        program: String,
    },
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
    if depth > 8 {
        return;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let base = path.parent().unwrap_or_else(|| Path::new("/"));
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line.strip_prefix("include ") {
            read_ld_so_conf_include(paths, base, pattern.trim(), depth + 1);
        } else {
            insert_existing_library_path(paths, line);
        }
    }
}

fn read_ld_so_conf_include(paths: &mut Vec<String>, base: &Path, pattern: &str, depth: usize) {
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
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if glob_file_name_matches(file_pattern, file_name) {
            read_ld_so_conf(paths, &path, depth);
        }
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

pub fn standard_dynamic_linker() -> Option<String> {
    standard_dynamic_linker_for(&LinkTarget::host())
}

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

fn elf_interpreter(path: &PathBuf) -> Result<Option<String>, LinkerConfigError> {
    const EI_CLASS: usize = 4;
    const EI_DATA: usize = 5;
    const ELFCLASS64: u8 = 2;
    const ELFDATA2LSB: u8 = 1;
    const PT_INTERP: u32 = 3;
    const ELF_HEADER_LEN: usize = 64;
    const PROGRAM_HEADER_LEN_64: usize = 56;

    let bytes = fs::read(path).map_err(|error| LinkerConfigError::Io {
        path: path.clone(),
        error,
    })?;
    if bytes.len() < ELF_HEADER_LEN
        || &bytes[0..4] != b"\x7fELF"
        || bytes[EI_CLASS] != ELFCLASS64
        || bytes[EI_DATA] != ELFDATA2LSB
    {
        return Err(LinkerConfigError::InvalidElf { path: path.clone() });
    }

    let phoff = read_u64(&bytes, 32)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    let phentsize = read_u16(&bytes, 54)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    let phnum = read_u16(&bytes, 56)
        .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
        as usize;
    if phentsize < PROGRAM_HEADER_LEN_64 {
        return Err(LinkerConfigError::InvalidElf { path: path.clone() });
    }

    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        let Some(p_type) = read_u32(&bytes, offset) else {
            return Err(LinkerConfigError::InvalidElf { path: path.clone() });
        };
        if p_type != PT_INTERP {
            continue;
        }
        let p_offset = read_u64(&bytes, offset + 8)
            .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
            as usize;
        let p_filesz = read_u64(&bytes, offset + 32)
            .ok_or_else(|| LinkerConfigError::InvalidElf { path: path.clone() })?
            as usize;
        let Some(slice) = bytes.get(p_offset..p_offset + p_filesz) else {
            return Err(LinkerConfigError::InvalidElf { path: path.clone() });
        };
        let nul = slice
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(slice.len());
        return String::from_utf8(slice[..nul].to_vec())
            .map(Some)
            .map_err(|_| LinkerConfigError::InvalidElf { path: path.clone() });
    }
    Ok(None)
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
