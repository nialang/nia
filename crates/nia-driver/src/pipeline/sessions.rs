// SPDX-License-Identifier: GPL-3.0-or-later
//! Reusable loader/compiler session state owned by the driver.

use std::fmt;

use nia_compiler_query::CompilerDatabase;
use nia_imports::ModuleMap;
use nia_loader_query::LoaderDatabase;
use nia_source::SourcePath;
use nia_target_config::{BuildProfile, CompilationMode, TargetConfig};
use nia_toolchain::RuntimeSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoaderKey {
    pub(super) entry_path: SourcePath,
    pub(super) package_root: Option<SourcePath>,
    pub(super) module_map: ModuleMap,
    pub(super) target: TargetConfig,
    pub(super) profile: BuildProfile,
    pub(super) compilation_mode: CompilationMode,
    pub(super) runtime: RuntimeSpec,
}

#[derive(Clone)]
pub(super) struct SessionLoader {
    pub(super) key: LoaderKey,
    pub(super) database: LoaderDatabase,
}

impl fmt::Debug for SessionLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLoader")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(super) struct SessionCompiler {
    pub(super) database: CompilerDatabase,
}

impl fmt::Debug for SessionCompiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionCompiler").finish_non_exhaustive()
    }
}
