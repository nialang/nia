// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental compiler query graph and frontend persistence contracts.
//!
//! This crate joins loader-owned source/module facts to semantic, executable,
//! and backend products inside one [`nia_query::QuerySession`]. Public
//! fingerprints identify relocatable persisted frontend products; session-local
//! compiler databases retain query ownership, invalidation, and diagnostics.
mod frontend_fingerprint;
mod program_diagnostic_bundle;
mod query;
mod signature_cache;

use nia_abi_check::AbiCheck;
use nia_backend_lower::BackendLowering;
use nia_body_ir::BodyIr;
use nia_const_check::ConstCheck;
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, Severity};
use nia_flow_check::FlowCheck;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ModuleGraphSnapshot;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_node_id::NodeOriginTable;
use nia_opt::OptimizationPolicy;
use nia_parser::ParseError;
use nia_provider_summary::ProviderSummary;
use nia_sema_ir::{SemanticFacts, SemanticUseTable};
use nia_source::{SourceIdentity, SourcePath, SourceVersion};
use nia_static_check::StaticCheck;
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;

pub use nia_body_check::{
    ProviderDemand, ProviderFactRevision, ProviderFactRevisionTransition, ProviderRequest,
};

pub use frontend_fingerprint::{
    FrontendCacheNamespace, FrontendCheckCertificateCacheKey, FrontendCheckInputFingerprint,
    FrontendCheckScope, FrontendExecutableValueRefEdgesCacheKey,
    FrontendExtensionValidationDiagnosticsCacheKey, FrontendFacadeFactsCacheKey,
    FrontendItemSignatureCacheKey, FrontendModuleDependenciesCacheKey,
    FrontendModuleMapFingerprint, FrontendProgramSourceFingerprint,
    FrontendProviderDemandPlanCacheKey, FrontendProviderSummaryCacheKey,
    FrontendPublicSurfaceFactsCacheKey, FrontendSignatureItemSignaturesCacheKey,
    FrontendSignatureTypeLoweringCacheKey, FrontendSignatureTypeResolutionCacheKey,
    FrontendSourceCacheKey, FrontendSyntaxCacheKey, ItemSignatureFingerprint,
    SourceContentFingerprint, SyntaxFingerprint, frontend_module_map_fingerprint,
    frontend_program_source_fingerprint, item_signature_fingerprint, source_content_fingerprint,
    syntax_fingerprint,
};

pub use nia_backend_lower::{BackendOptimizationChange, BackendOptimizationReport};
pub use nia_timing::TimingMode;
pub use query::{CompileRequest, CompilerDatabase};

/// Converts a query-engine failure into a compiler-owned diagnostic.
pub fn query_error_diagnostic(error: nia_query::QueryError) -> Diagnostic {
    query::query_error_diagnostic(error)
}

/// Completion-order view over parallel backend module finalization.
///
/// Readiness positions are checked against query completion positions before
/// modules are exposed, keeping deterministic collector ownership explicit.
pub struct BackendFinalizationSchedule<'borrow, 'stream, 'executor> {
    completions: &'borrow mut nia_query::QueryCompletionStream<
        'stream,
        'executor,
        nia_query::QueryResult<nia_backend_lower::BackendModuleFinalization>,
    >,
    collector: Option<nia_backend_lower::BackendModuleFinalizationCollector>,
    readiness: nia_backend_ir::BackendModuleReadiness,
}

impl<'borrow, 'stream, 'executor> BackendFinalizationSchedule<'borrow, 'stream, 'executor> {
    pub(crate) fn new(
        completions: &'borrow mut nia_query::QueryCompletionStream<
            'stream,
            'executor,
            nia_query::QueryResult<nia_backend_lower::BackendModuleFinalization>,
        >,
        collector: nia_backend_lower::BackendModuleFinalizationCollector,
    ) -> Self {
        let readiness = collector.take_readiness();
        Self {
            completions,
            collector: Some(collector),
            readiness,
        }
    }

    /// Returns the shared store receiving finalized modules.
    pub fn module_store(&self) -> std::sync::Arc<nia_backend_ir::BackendModuleStore> {
        self.collector
            .as_ref()
            .expect("backend finalization collector")
            .module_store()
    }

    /// Returns the module-to-owner directory used during publication.
    pub fn owner_directory(&self) -> std::sync::Arc<nia_backend_ir::BackendModuleOwnerDirectory> {
        self.collector
            .as_ref()
            .expect("backend finalization collector")
            .owner_directory()
    }

    /// Waits for and publishes the next completed backend module.
    pub fn wait_next(
        &mut self,
    ) -> nia_query::QueryResult<Option<nia_backend_ir::BackendModuleReady>> {
        let Some((position, finalization)) = self.completions.wait_next() else {
            return Ok(None);
        };
        let finalization = finalization?;
        self.collector
            .as_mut()
            .expect("backend finalization collector")
            .push(position, finalization);
        let ready = self
            .readiness
            .wait_next()
            .expect("backend finalization publication must produce readiness");
        assert_eq!(
            ready.position(),
            position,
            "Nia ICE: backend readiness must match query completion position"
        );
        Ok(Some(ready))
    }

    /// Drains remaining completions and returns the complete lowering product.
    pub fn finish(mut self) -> nia_query::QueryResult<BackendLowering> {
        while self.wait_next()?.is_some() {}
        Ok(self
            .collector
            .take()
            .expect("backend finalization collector")
            .finish())
    }
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
    ) -> Self {
        assert!(
            matches!(
                revision.transition_from(reset_revision),
                ProviderFactRevisionTransition::Unchanged
                    | ProviderFactRevisionTransition::Advanced
            ),
            "Nia ICE: provider fact reset revision must belong to the current lineage"
        );
        Self {
            revision,
            reset_revision,
            demands: demands.into_iter().collect(),
        }
    }

    /// Creates an empty snapshot at `revision`.
    pub fn empty(revision: ProviderFactRevision) -> Self {
        Self::new(revision, revision, std::iter::empty())
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
///
/// Implementations must return facts from the advertised query session. Stable
/// source identities cross persistence boundaries; module ids and node stores
/// remain scoped to the current loaded graph.
#[allow(missing_docs)]
pub trait LoaderFactProvider: Send + Sync {
    fn query_session(&self) -> Option<nia_query::QuerySession>;
    fn provider_facts(&self) -> nia_query::QueryResult<ProviderFactSnapshot>;
    fn update_provider_demands(
        &self,
        demands: Vec<ProviderDemand>,
    ) -> nia_query::QueryResult<ProviderGraphUpdate>;
    fn settle_provider_demands(&self) -> nia_query::QueryResult<()> {
        Ok(())
    }
    fn node_store(&self) -> nia_node_id::NodeStore;
    fn module_graph(&self) -> nia_query::QueryResult<ModuleGraphSnapshot>;
    fn loaded_module_source_identities(&self) -> nia_query::QueryResult<Vec<SourceIdentity>>;
    fn module_path(&self, module_id: ModuleId) -> nia_query::QueryResult<Option<SourcePath>>;
    fn module_source_version(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<SourceVersion>>;
    fn module_source_fingerprint(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<(SourceContentFingerprint, usize)>>;
    fn module_provider_summary(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<ProviderSummary>>;
    fn module_public_surface_facts(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<nia_defs::PublicSurfaceModuleFacts>> {
        let Some(tree) =
            self.active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
        else {
            return Ok(None);
        };
        let symbols = self.symbols();
        let defs = nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
            module_id,
            &tree,
            &self.node_store(),
            &symbols,
        );
        Ok(Some(nia_defs::PublicSurfaceModuleFacts::from_defs(&defs)))
    }
    fn module_origins(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<NodeOriginTable>>;
    fn module_parse_errors(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<Vec<ParseError>>>;
    fn module_item_tree(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<ModuleItemTree>>;
    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> nia_query::QueryResult<Option<ActiveModuleItemTree>>;
    fn load_diagnostics(&self) -> nia_query::QueryResult<ProgramDiagnosticBundles>;
    fn symbols(&self) -> SymbolTable;
    fn target(&self) -> TargetConfig;
    fn runtime(&self) -> RuntimeModel;
    fn toolchain_identity(&self) -> nia_toolchain::ToolchainIdentityFingerprint {
        nia_toolchain::ToolchainIdentityFingerprint::current()
    }
}

/// Complete loader snapshot usable as an untracked compiler input.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct LoadedProgram {
    pub graph: ModuleGraphSnapshot,
    pub provider_fact_revision: ProviderFactRevision,
    pub symbols: SymbolTable,
    pub target: TargetConfig,
    pub runtime: RuntimeModel,
    pub toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    pub modules: Vec<LoadedModule>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

impl LoaderFactProvider for LoadedProgram {
    fn query_session(&self) -> Option<nia_query::QuerySession> {
        None
    }

    fn provider_facts(&self) -> nia_query::QueryResult<ProviderFactSnapshot> {
        Ok(ProviderFactSnapshot::empty(self.provider_fact_revision))
    }

    fn update_provider_demands(
        &self,
        _demands: Vec<ProviderDemand>,
    ) -> nia_query::QueryResult<ProviderGraphUpdate> {
        Ok(ProviderGraphUpdate::Stable)
    }

    fn node_store(&self) -> nia_node_id::NodeStore {
        self.modules
            .first()
            .map(|module| module.origins.node_store().clone())
            .unwrap_or_default()
    }

    fn module_graph(&self) -> nia_query::QueryResult<ModuleGraphSnapshot> {
        Ok(self.graph.clone())
    }

    fn loaded_module_source_identities(&self) -> nia_query::QueryResult<Vec<SourceIdentity>> {
        Ok(self
            .modules
            .iter()
            .map(|module| module.source_identity.clone())
            .collect())
    }

    fn module_path(&self, module_id: ModuleId) -> nia_query::QueryResult<Option<SourcePath>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.path.clone()))
    }

    fn module_source_version(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<SourceVersion>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.source_version))
    }

    fn module_source_fingerprint(
        &self,
        _module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<(SourceContentFingerprint, usize)>> {
        Ok(None)
    }

    fn module_provider_summary(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<ProviderSummary>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.provider_summary.clone()))
    }

    fn module_origins(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<NodeOriginTable>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.origins.clone()))
    }

    fn module_parse_errors(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<Vec<ParseError>>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.parse_errors.clone()))
    }

    fn module_item_tree(
        &self,
        module_id: ModuleId,
    ) -> nia_query::QueryResult<Option<ModuleItemTree>> {
        Ok(self
            .modules
            .iter()
            .find(|module| module.id == module_id)
            .map(|module| module.item_tree.clone()))
    }

    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> nia_query::QueryResult<Option<ActiveModuleItemTree>> {
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

    fn load_diagnostics(&self) -> nia_query::QueryResult<ProgramDiagnosticBundles> {
        Ok(ProgramDiagnosticBundles::from_diagnostics(
            self.diagnostics.clone(),
        ))
    }

    fn symbols(&self) -> SymbolTable {
        self.symbols.clone()
    }

    fn target(&self) -> TargetConfig {
        self.target.clone()
    }

    fn runtime(&self) -> RuntimeModel {
        self.runtime
    }

    fn toolchain_identity(&self) -> nia_toolchain::ToolchainIdentityFingerprint {
        self.toolchain_identity
    }
}

/// Runtime model contributing to frontend and executable query identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeModel {
    /// Compile without runtime support.
    #[default]
    Bare,
    /// Compile a freestanding executable with its runtime resources.
    FreestandingExecutable,
}

/// Loaded source/module facts for one module.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct LoadedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub source_identity: SourceIdentity,
    pub source_version: SourceVersion,
    pub item_tree: ModuleItemTree,
    pub active_item_tree: ActiveModuleItemTree,
    pub provider_summary: ProviderSummary,
    pub origins: NodeOriginTable,
    pub parse_errors: Vec<ParseError>,
}

/// User-visible result of checking a program.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct CheckedProgram {
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub diagnostics: Vec<ProgramDiagnostic>,
    checked_body_count: usize,
    reachable_body_count: usize,
}

impl CheckedProgram {
    /// Returns the number of bodies checked for semantic validity.
    pub fn checked_body_count(&self) -> usize {
        self.checked_body_count
    }

    /// Returns the number of checked bodies reachable in this report.
    pub fn reachable_body_count(&self) -> usize {
        self.reachable_body_count
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgramAnalysis {
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

impl CheckedProgramAnalysis {
    pub fn into_report(self) -> CheckedProgram {
        let checked_body_count = self
            .modules
            .iter()
            .map(|module| module.body_ir.function_bodies.len())
            .sum();
        CheckedProgram {
            graph: self.graph,
            optimization: self.optimization,
            diagnostics: self.diagnostics,
            checked_body_count,
            reachable_body_count: checked_body_count,
        }
    }
}

/// Checked semantic products needed before backend lowering begins.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct CodegenPreparation {
    pub type_store: std::sync::Arc<nia_ty::TypeStore>,
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    pub monomorphization: std::sync::Arc<Monomorphization>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

/// Complete checked and backend-lowered compiler product.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct CodegenProgram {
    pub type_store: std::sync::Arc<nia_ty::TypeStore>,
    pub graph: ModuleGraphSnapshot,
    pub optimization: OptimizationPolicy,
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    pub monomorphization: std::sync::Arc<Monomorphization>,
    pub backend_lowering: std::sync::Arc<BackendLowering>,
    pub diagnostics: Vec<ProgramDiagnostic>,
}

/// Diagnostic paired with its stable source path.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramDiagnostic {
    /// Source path owning the diagnostic.
    pub path: SourcePath,
    /// Structured diagnostic payload.
    pub diagnostic: Diagnostic,
}

impl ProgramDiagnostic {
    /// Returns whether the diagnostic has error severity.
    pub fn is_error(&self) -> bool {
        self.diagnostic.severity == Severity::Error
    }

    /// Returns whether the diagnostic has warning severity.
    pub fn is_warning(&self) -> bool {
        self.diagnostic.severity == Severity::Warning
    }
}

/// Store-owned diagnostic bundles grouped by source without flattening eagerly.
#[derive(Clone)]
pub struct ProgramDiagnosticBundles {
    store: std::sync::Arc<nia_diagnostic::DiagnosticStore>,
    bundles: std::sync::Arc<[SourceDiagnosticBundle]>,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceDiagnosticBundle {
    path: SourcePath,
    diagnostics: nia_diagnostic::DiagnosticBundle,
}

impl ProgramDiagnosticBundles {
    /// Groups ordered diagnostics in a fresh diagnostic store.
    pub fn from_diagnostics(diagnostics: Vec<ProgramDiagnostic>) -> Self {
        let store = std::sync::Arc::new(nia_diagnostic::DiagnosticStore::new());
        Self::from_diagnostics_in(store, diagnostics)
    }

    /// Groups ordered diagnostics in the supplied owning store.
    pub fn from_diagnostics_in(
        store: std::sync::Arc<nia_diagnostic::DiagnosticStore>,
        diagnostics: Vec<ProgramDiagnostic>,
    ) -> Self {
        let mut diagnostics = diagnostics.into_iter().peekable();
        let mut bundles = Vec::new();
        while let Some(ProgramDiagnostic { path, diagnostic }) = diagnostics.next() {
            let mut source_diagnostics = vec![diagnostic];
            while let Some(next) = diagnostics.next_if(|next| next.path == path) {
                source_diagnostics.push(next.diagnostic);
            }
            bundles.push(SourceDiagnosticBundle {
                path,
                diagnostics: store.bundle(source_diagnostics),
            });
        }
        Self {
            store,
            bundles: bundles.into(),
        }
    }

    /// Wraps one already store-owned source bundle.
    pub fn from_source_bundle(
        store: std::sync::Arc<nia_diagnostic::DiagnosticStore>,
        path: SourcePath,
        diagnostics: nia_diagnostic::DiagnosticBundle,
    ) -> Self {
        if store.diagnostics(&diagnostics).is_none() {
            panic!("Nia ICE: program diagnostic bundle has a foreign store owner");
        }
        let bundles = if diagnostics.is_empty() {
            Vec::new()
        } else {
            vec![SourceDiagnosticBundle { path, diagnostics }]
        };
        Self {
            store,
            bundles: bundles.into(),
        }
    }

    /// Appends bundles that share the same diagnostic store owner.
    pub fn append(&self, other: &Self) -> Self {
        if !std::sync::Arc::ptr_eq(&self.store, &other.store) {
            panic!("Nia ICE: cannot append program diagnostics from different stores");
        }
        Self {
            store: self.store.clone(),
            bundles: self
                .bundles
                .iter()
                .chain(other.bundles.iter())
                .cloned()
                .collect::<Vec<_>>()
                .into(),
        }
    }

    /// Materializes source-qualified diagnostics in bundle order.
    pub fn to_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.bundles
            .iter()
            .flat_map(|bundle| {
                self.store
                    .diagnostics(&bundle.diagnostics)
                    .unwrap_or_else(|| {
                        panic!("Nia ICE: program diagnostic bundle has a foreign store owner")
                    })
                    .iter()
                    .cloned()
                    .map(|diagnostic| ProgramDiagnostic {
                        path: bundle.path.clone(),
                        diagnostic,
                    })
            })
            .collect()
    }

    /// Returns whether the collection contains no source bundles.
    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }
}

impl std::fmt::Debug for ProgramDiagnosticBundles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ProgramDiagnosticBundles")
            .field(&self.to_diagnostics())
            .finish()
    }
}

impl PartialEq for ProgramDiagnosticBundles {
    fn eq(&self, other: &Self) -> bool {
        self.to_diagnostics() == other.to_diagnostics()
    }
}

/// Returns whether any program diagnostic has error severity.
pub fn has_error_diagnostics(diagnostics: &[ProgramDiagnostic]) -> bool {
    diagnostics.iter().any(ProgramDiagnostic::is_error)
}

/// Per-module checked products shared by later compiler queries.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct CheckedModule {
    pub id: ModuleId,
    pub path: SourcePath,
    pub defs: std::sync::Arc<DefCollection>,
    pub definition_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub type_resolution: std::sync::Arc<TypeResolution>,
    pub type_lowering: std::sync::Arc<TypeLowering>,
    pub value_resolution: std::sync::Arc<ValueResolution>,
    pub local_resolution: std::sync::Arc<LocalResolution>,
    pub type_normalization: std::sync::Arc<TypeNormalization>,
    pub const_eval: std::sync::Arc<ConstCheck>,
    pub static_check: std::sync::Arc<StaticCheck>,
    pub layouts: std::sync::Arc<Layouts>,
    pub abi_check: std::sync::Arc<AbiCheck>,
    pub flow_check: std::sync::Arc<FlowCheck>,
    pub body_ir: std::sync::Arc<BodyIr>,
    pub semantic_uses: std::sync::Arc<SemanticUseTable>,
    pub semantic_facts: std::sync::Arc<SemanticFacts>,
    pub provider_demands: std::sync::Arc<std::collections::HashSet<ProviderDemand>>,
    pub executable_reachable_globals: Option<std::collections::HashSet<GlobalDefId>>,
    pub executable_reachable_structs:
        Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    pub executable_reachable_unions: Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    pub executable_type_only: bool,
    pub(crate) body_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) frontend_diagnostics: Vec<nia_diagnostic::DiagnosticBundle>,
    pub(crate) resolution_diagnostics: Vec<nia_diagnostic::DiagnosticBundle>,
    pub(crate) item_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) const_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) static_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) layout_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) abi_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) flow_diagnostics: nia_diagnostic::DiagnosticBundle,
}

pub(crate) fn module_diagnostics(
    path: &SourcePath,
    diagnostics: &[Diagnostic],
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path.clone(),
            diagnostic,
        })
        .collect()
}
