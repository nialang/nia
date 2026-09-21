// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler database input configuration.

use super::*;

/// Loader facts and session-stable policies used to create or update a compiler database.
#[derive(Clone)]
pub struct CompileRequest {
    pub(super) loader_facts: Arc<dyn crate::LoaderFactProvider>,
    /// Optimization level contributing to executable query products.
    pub optimization: NiaOptimizationLevel,
    /// Compiler timing collection policy.
    pub timings: TimingMode,
    /// Definition-root scope used by executable and package code generation.
    pub codegen_scope: crate::CodegenScope,
    /// Canonical identity of the current source package for package-qualified
    /// symbols. Standalone source compilations derive an anonymous identity
    /// from their stable source root.
    pub current_package: Option<PackageId>,
    pub(super) frontend_cache_dir: Option<PathBuf>,
    pub(super) verify_frontend_cache: bool,
}

impl CompileRequest {
    /// Creates a request from a loader fact provider with caching disabled.
    pub fn new(loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loader_facts);
        Self {
            loader_facts,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            codegen_scope: crate::CodegenScope::Entry,
            current_package: None,
            frontend_cache_dir: None,
            verify_frontend_cache: false,
        }
    }

    /// Selects the executable optimization level.
    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    /// Selects compiler timing collection.
    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    /// Selects whether code generation starts from an entry or the complete
    /// concrete definition inventory of the current package.
    pub fn with_codegen_scope(mut self, scope: crate::CodegenScope) -> Self {
        self.codegen_scope = scope;
        self
    }

    /// Binds current-source linkage to a canonical package identity.
    pub fn with_current_package(mut self, package: Option<PackageId>) -> Self {
        self.current_package = package;
        self
    }

    /// Selects the persistent frontend cache root for this query session.
    pub fn with_frontend_cache_dir(mut self, frontend_cache_dir: Option<PathBuf>) -> Self {
        self.frontend_cache_dir = frontend_cache_dir;
        self
    }

    /// Enables recomputation and comparison of otherwise reusable frontend entries.
    pub fn with_frontend_cache_verification(mut self, verify: bool) -> Self {
        self.verify_frontend_cache = verify;
        self
    }

    #[cfg(test)]
    pub(super) fn with_loader_facts(
        mut self,
        loader_facts: impl crate::LoaderFactProvider + 'static,
    ) -> Self {
        self.loader_facts = Arc::new(loader_facts);
        self
    }
}
