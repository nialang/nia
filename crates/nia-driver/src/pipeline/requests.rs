// SPDX-License-Identifier: GPL-3.0-or-later
//! Public input contracts for driver pipeline stages.

use std::path::PathBuf;

use nia_compiler_query::TimingMode;
use nia_imports::ModuleMap;
use nia_linker::LinkOptions;
use nia_opt::NiaOptimizationLevel;
use nia_package_metadata::PackageId;
use nia_source::SourcePath;
use nia_target_config::{BuildProfile, CompilationMode};
use nia_toolchain::RuntimeSpec;

/// Frontend and semantic options shared by driver requests.
#[derive(Debug, Clone)]
pub struct CheckRequest {
    /// Entry source path.
    pub entry_path: SourcePath,
    /// Optional package root source path shared by the entry module.
    pub package_root: Option<SourcePath>,
    /// Canonical identity used for current-package linkage when publishing
    /// reusable native objects.
    pub current_package: Option<PackageId>,
    /// Explicit module-name to source-path mappings.
    pub module_map: ModuleMap,
    /// Nia optimization level.
    pub optimization: NiaOptimizationLevel,
    /// Timing collection mode.
    pub timings: TimingMode,
    /// Validated runtime startup selection.
    pub runtime: RuntimeSpec,
    /// Build profile used for conditional source selection.
    pub profile: BuildProfile,
    /// Whether test-only source participates in compilation.
    pub compilation_mode: CompilationMode,
}

impl CheckRequest {
    /// Creates a request for an entry path with default options.
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self::from_source_path(SourcePath::new(entry_path.into()))
    }

    /// Creates a request from an already normalized source path.
    pub fn from_source_path(entry_path: SourcePath) -> Self {
        Self {
            entry_path,
            package_root: None,
            current_package: None,
            module_map: ModuleMap::default(),
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            runtime: RuntimeSpec::Bare,
            profile: BuildProfile::default(),
            compilation_mode: CompilationMode::default(),
        }
    }

    /// Selects a separate `pkg.nia` package root for this entry.
    pub fn with_package_root(mut self, package_root: SourcePath) -> Self {
        self.package_root = Some(package_root);
        self
    }

    /// Binds current-source linkage to the package identity that will own the
    /// emitted native objects.
    pub fn with_current_package(mut self, package: PackageId) -> Self {
        self.current_package = Some(package);
        self
    }

    /// Supplies explicit module mappings.
    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    /// Selects the Nia optimization level.
    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    /// Selects timing collection for this request.
    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    /// Selects runtime startup semantics.
    pub fn with_runtime(mut self, runtime: RuntimeSpec) -> Self {
        self.runtime = runtime;
        self
    }

    /// Selects the build profile used for conditional source selection.
    pub fn with_profile(mut self, profile: BuildProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Selects whether test-only source participates in compilation.
    pub fn with_compilation_mode(mut self, mode: CompilationMode) -> Self {
        self.compilation_mode = mode;
        self
    }
}

/// Request to emit LLVM IR from a check request.
#[derive(Debug, Clone)]
pub struct EmitLlvmRequest {
    /// Underlying check and semantic options.
    pub check: CheckRequest,
}

impl EmitLlvmRequest {
    /// Wraps a check request.
    pub fn new(check: CheckRequest) -> Self {
        Self { check }
    }
}

/// Request to emit native objects from a check request.
#[derive(Debug, Clone)]
pub struct EmitObjectRequest {
    /// Underlying check and semantic options.
    pub check: CheckRequest,
}

impl EmitObjectRequest {
    /// Wraps a check request.
    pub fn new(check: CheckRequest) -> Self {
        Self { check }
    }
}

/// Request to write native objects to one file or a directory.
#[derive(Debug, Clone)]
pub struct WriteObjectRequest {
    /// Underlying check and semantic options.
    pub check: CheckRequest,
    /// Destination policy.
    pub output: ObjectOutput,
}

impl WriteObjectRequest {
    /// Creates a write request.
    pub fn new(check: CheckRequest, output: ObjectOutput) -> Self {
        Self { check, output }
    }
}

/// Request to link an executable from a check request.
#[derive(Debug, Clone)]
pub struct LinkExecutableRequest {
    /// Underlying check and semantic options.
    pub check: CheckRequest,
    /// Executable destination path.
    pub output: PathBuf,
    /// Linker options.
    pub link_options: LinkOptions,
    /// Cross-module optimization applied only while producing this executable.
    pub link_time_optimization: LinkTimeOptimization,
}

/// Final executable cross-module optimization policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkTimeOptimization {
    /// Emit and link independent native objects.
    #[default]
    Off,
    /// Build a ThinLTO summary index and run parallel importing backends.
    Thin,
    /// Merge regular-LTO modules and optimize the complete linkage unit.
    Full,
}

impl LinkExecutableRequest {
    /// Creates a link request with default linker options.
    pub fn new(check: CheckRequest, output: impl Into<PathBuf>) -> Self {
        Self {
            check,
            output: output.into(),
            link_options: LinkOptions::default(),
            link_time_optimization: LinkTimeOptimization::Off,
        }
    }

    /// Replaces the linker options.
    pub fn with_link_options(mut self, link_options: LinkOptions) -> Self {
        self.link_options = link_options;
        self
    }

    /// Selects the final executable cross-module optimization policy.
    pub fn with_link_time_optimization(mut self, policy: LinkTimeOptimization) -> Self {
        self.link_time_optimization = policy;
        self
    }
}

/// Destination policy for native object emission.
#[derive(Debug, Clone)]
pub enum ObjectOutput {
    /// Emit one combined object file.
    Single(PathBuf),
    /// Emit one object file per module into a directory.
    Directory(PathBuf),
}
