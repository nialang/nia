// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable contracts between source loading and compiler queries.
//!
//! This crate owns the loader-facing data model. The loader produces these
//! facts and the compiler consumes them; neither side needs to depend on the
//! other's orchestration implementation.

mod frontend_fingerprint;
mod program_diagnostics;

use std::sync::Arc;

use nia_defs::PublicSurfaceModuleFacts;
use nia_imports::ModuleGraphSnapshot;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_node_id::{NodeOriginTable, NodeStore};
use nia_parser::ParseError;
use nia_provider_summary::ProviderSummary;
use nia_query::{QueryResult, QuerySession};
use nia_source::{SourceIdentity, SourcePath, SourceVersion};
use nia_symbol_table::SymbolTable;
use nia_target_config::{BuildProfile, CompilationMode, TargetConfig};
use nia_toolchain::ToolchainIdentityFingerprint;

pub use frontend_fingerprint::*;
pub use nia_provider_summary::{
    ProviderDemand, ProviderFactRevision, ProviderFactRevisionTransition, ProviderRequest,
};
pub use nia_toolchain::RuntimeSpec;
pub use program_diagnostics::{ProgramDiagnostic, ProgramDiagnosticBundles};

/// Returns whether any program diagnostic has error severity.
pub fn has_error_diagnostics(diagnostics: &[ProgramDiagnostic]) -> bool {
    diagnostics.iter().any(ProgramDiagnostic::is_error)
}

/// Loader item-tree projection required by a compiler query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveModuleItemTreeFactKind {
    /// Items contributing to one signature family.
    Signature(nia_item_tree::SignatureItemSet),
    /// Items required to evaluate constant signatures.
    ConstSignature,
    /// Complete target-active module tree.
    Full,
}

/// Immutable provider-demand facts and their revision lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFactSnapshot {
    revision: ProviderFactRevision,
    reset_revision: ProviderFactRevision,
    demands: std::collections::HashSet<ProviderDemand>,
}

impl ProviderFactSnapshot {
    /// Creates a snapshot and verifies that its reset revision shares a lineage.
    pub fn new(
        revision: ProviderFactRevision,
        reset_revision: ProviderFactRevision,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> nia_ice::IceResult<Self> {
        if !matches!(
            revision.transition_from(reset_revision),
            ProviderFactRevisionTransition::Unchanged | ProviderFactRevisionTransition::Advanced
        ) {
            return Err(nia_ice::Ice::new(
                "provider fact reset revision must belong to the current lineage",
            ));
        }
        Ok(Self {
            revision,
            reset_revision,
            demands: demands.into_iter().collect(),
        })
    }

    /// Creates an empty snapshot at `revision`.
    pub fn empty(revision: ProviderFactRevision) -> Self {
        Self {
            revision,
            reset_revision: revision,
            demands: std::collections::HashSet::new(),
        }
    }

    /// Returns the current provider-fact revision.
    pub fn revision(&self) -> ProviderFactRevision {
        self.revision
    }

    /// Returns the revision at which the current demand set was reset.
    pub fn reset_revision(&self) -> ProviderFactRevision {
        self.reset_revision
    }

    /// Returns the deduplicated provider demands in this snapshot.
    pub fn demands(&self) -> &std::collections::HashSet<ProviderDemand> {
        &self.demands
    }
}

/// Effect of applying a compiler provider-demand batch to the loader graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderGraphUpdate {
    /// No loader facts changed.
    Stable,
    /// Provider discovery changed the graph.
    Changed {
        /// Whether already resolved body facts must be recomputed.
        invalidates_resolved_body_facts: bool,
    },
}

/// Loader-owned facts consumed by the incremental compiler database.
pub trait LoaderFactProvider: Send + Sync {
    fn query_session(&self) -> Option<QuerySession>;
    fn provider_facts(&self) -> QueryResult<ProviderFactSnapshot>;
    fn update_provider_demands(
        &self,
        demands: Vec<ProviderDemand>,
    ) -> QueryResult<ProviderGraphUpdate>;
    fn settle_provider_demands(&self) -> QueryResult<()> {
        Ok(())
    }
    fn node_store(&self) -> NodeStore;
    fn module_graph(&self) -> QueryResult<ModuleGraphSnapshot>;
    fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>>;
    fn module_path(&self, module_id: nia_ids::ModuleId) -> QueryResult<Option<SourcePath>>;
    fn module_source_version(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<SourceVersion>>;
    fn module_source_fingerprint(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<(SourceContentFingerprint, usize)>>;
    fn module_source_text(&self, _module_id: nia_ids::ModuleId) -> QueryResult<Option<Arc<str>>> {
        Ok(None)
    }
    fn module_provider_summary(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<ProviderSummary>>;
    fn module_public_surface_facts(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<PublicSurfaceModuleFacts>> {
        let Some(tree) =
            self.active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
        else {
            return Ok(None);
        };
        let defs = nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
            module_id,
            &tree,
            &self.node_store(),
            &self.symbols(),
        )?;
        Ok(Some(PublicSurfaceModuleFacts::from_defs(&defs)))
    }
    fn module_origins(&self, module_id: nia_ids::ModuleId) -> QueryResult<Option<NodeOriginTable>>;
    fn module_parse_errors(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<Vec<ParseError>>>;
    fn module_item_tree(&self, module_id: nia_ids::ModuleId)
    -> QueryResult<Option<ModuleItemTree>>;
    fn active_module_item_tree(
        &self,
        module_id: nia_ids::ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> QueryResult<Option<ActiveModuleItemTree>>;
    fn load_diagnostics(&self) -> QueryResult<ProgramDiagnosticBundles>;
    fn symbols(&self) -> SymbolTable;
    fn target(&self) -> TargetConfig;
    fn profile(&self) -> BuildProfile {
        BuildProfile::default()
    }
    fn compilation_mode(&self) -> CompilationMode {
        CompilationMode::default()
    }
    fn runtime(&self) -> RuntimeSpec;
    fn toolchain_identity(&self) -> ToolchainIdentityFingerprint {
        ToolchainIdentityFingerprint::current()
    }
}

/// Complete loader snapshot usable as an untracked compiler input.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProgram {
    pub graph: ModuleGraphSnapshot,
    pub provider_fact_revision: ProviderFactRevision,
    pub symbols: SymbolTable,
    pub target: TargetConfig,
    pub profile: BuildProfile,
    pub compilation_mode: CompilationMode,
    pub runtime: RuntimeSpec,
    pub toolchain_identity: ToolchainIdentityFingerprint,
    pub modules: Vec<LoadedModule>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

impl LoaderFactProvider for LoadedProgram {
    fn query_session(&self) -> Option<QuerySession> {
        None
    }
    fn provider_facts(&self) -> QueryResult<ProviderFactSnapshot> {
        Ok(ProviderFactSnapshot::empty(self.provider_fact_revision))
    }
    fn update_provider_demands(
        &self,
        _demands: Vec<ProviderDemand>,
    ) -> QueryResult<ProviderGraphUpdate> {
        Ok(ProviderGraphUpdate::Stable)
    }
    fn node_store(&self) -> NodeStore {
        self.modules
            .first()
            .map(|module| module.origins.node_store().clone())
            .unwrap_or_default()
    }
    fn module_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        Ok(self.graph.clone())
    }
    fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>> {
        Ok(self
            .modules
            .iter()
            .map(|module| module.source_identity.clone())
            .collect())
    }
    fn module_path(&self, module_id: nia_ids::ModuleId) -> QueryResult<Option<SourcePath>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.path.clone()))
    }
    fn module_source_version(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<SourceVersion>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.source_version))
    }
    fn module_source_fingerprint(
        &self,
        _module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<(SourceContentFingerprint, usize)>> {
        Ok(None)
    }
    fn module_source_text(&self, module_id: nia_ids::ModuleId) -> QueryResult<Option<Arc<str>>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.source_text.clone()))
    }
    fn module_provider_summary(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<ProviderSummary>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.provider_summary.clone()))
    }
    fn module_origins(&self, module_id: nia_ids::ModuleId) -> QueryResult<Option<NodeOriginTable>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.origins.clone()))
    }
    fn module_parse_errors(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<Vec<ParseError>>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.parse_errors.clone()))
    }
    fn module_item_tree(
        &self,
        module_id: nia_ids::ModuleId,
    ) -> QueryResult<Option<ModuleItemTree>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.item_tree.clone()))
    }
    fn active_module_item_tree(
        &self,
        module_id: nia_ids::ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> QueryResult<Option<ActiveModuleItemTree>> {
        let Some(tree) = self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| &module.active_item_tree)
        else {
            return Ok(None);
        };
        Ok(Some(match kind {
            ActiveModuleItemTreeFactKind::Signature(set) => tree.signature_items(set),
            ActiveModuleItemTreeFactKind::ConstSignature => tree.const_signature_items(),
            ActiveModuleItemTreeFactKind::Full => tree.clone(),
        }))
    }
    fn load_diagnostics(&self) -> QueryResult<ProgramDiagnosticBundles> {
        Ok(ProgramDiagnosticBundles::from_diagnostics(
            self.diagnostics.clone(),
        )?)
    }
    fn symbols(&self) -> SymbolTable {
        self.symbols.clone()
    }
    fn target(&self) -> TargetConfig {
        self.target.clone()
    }
    fn profile(&self) -> BuildProfile {
        self.profile
    }
    fn compilation_mode(&self) -> CompilationMode {
        self.compilation_mode
    }
    fn runtime(&self) -> RuntimeSpec {
        self.runtime.clone()
    }
    fn toolchain_identity(&self) -> ToolchainIdentityFingerprint {
        self.toolchain_identity
    }
}

/// Loaded source/module facts for one module.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModule {
    pub id: nia_ids::ModuleId,
    pub path: SourcePath,
    pub source_identity: SourceIdentity,
    pub source_version: SourceVersion,
    pub source_text: Arc<str>,
    pub item_tree: ModuleItemTree,
    pub active_item_tree: ActiveModuleItemTree,
    pub provider_summary: ProviderSummary,
    pub origins: NodeOriginTable,
    pub parse_errors: Vec<ParseError>,
}
