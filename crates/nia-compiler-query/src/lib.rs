// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental compiler query graph and frontend persistence contracts.
//!
//! This crate joins loader-owned source/module facts to semantic, executable,
//! and backend products inside one [`nia_query::QuerySession`]. Public
//! fingerprints identify relocatable persisted frontend products; session-local
//! compiler databases retain query ownership, invalidation, and diagnostics.
mod program_diagnostic_bundle;
mod query;
mod signature_cache;

use nia_abi_check::AbiCheck;
use nia_backend_lower::BackendLowering;
use nia_body_ir::BodyIr;
use nia_const_check::ConstCheck;
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
use nia_flow_check::FlowCheck;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ModuleGraphSnapshot;
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_monomorphize::Monomorphization;
use nia_opt::OptimizationPolicy;
use nia_sema_ir::{SemanticFacts, SemanticUseTable};
use nia_source::SourcePath;
use nia_static_check::StaticCheck;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;

pub use nia_loader_contract::{
    ActiveModuleItemTreeFactKind, LoadedModule, LoadedProgram, LoaderFactProvider,
    ProgramDiagnostic, ProgramDiagnosticBundles, ProviderDemand, ProviderFactRevision,
    ProviderFactRevisionTransition, ProviderFactSnapshot, ProviderGraphUpdate, ProviderRequest,
    has_error_diagnostics,
};

pub use nia_loader_contract::{
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
    frontend_module_map_fingerprint_with_package_root, frontend_program_source_fingerprint,
    item_signature_fingerprint, source_content_fingerprint, syntax_fingerprint,
};

pub use nia_backend_ir::BackendFunctionStats;
pub use nia_backend_lower::{BackendOptimizationChange, BackendOptimizationReport};
pub use nia_timing::TimingMode;
pub use query::{
    CompileRequest, CompilerDatabase, StableDefinitionIndex, StableDefinitionPackageResolver,
    StableDefinitionResolver, StableModuleIdentity, StableModuleIndex, StableModulePackageResolver,
};

pub use nia_loader_contract::RuntimeSpec;

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
    collector: nia_backend_lower::BackendModuleFinalizationCollector,
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
        readiness: nia_backend_ir::BackendModuleReadiness,
    ) -> Self {
        Self {
            completions,
            collector,
            readiness,
        }
    }

    /// Returns the shared store receiving finalized modules.
    pub fn module_store(&self) -> std::sync::Arc<nia_backend_ir::BackendModuleStore> {
        self.collector.module_store()
    }

    /// Returns the module-to-owner directory used during publication.
    pub fn owner_directory(&self) -> std::sync::Arc<nia_backend_ir::BackendModuleOwnerDirectory> {
        self.collector.owner_directory()
    }

    /// Waits for and publishes the next completed backend module.
    pub fn wait_next(
        &mut self,
    ) -> nia_query::QueryResult<Option<nia_backend_ir::BackendModuleReady>> {
        let Some((position, finalization)) = self.completions.wait_next()? else {
            return Ok(None);
        };
        let finalization = finalization?;
        self.collector.push(position, finalization)?;
        let Some(ready) = self.readiness.wait_next() else {
            return Err(nia_query::QueryError::internal(
                "backend finalization publication did not produce readiness",
            ));
        };
        if ready.position() != position {
            return Err(nia_query::QueryError::internal(format!(
                "backend readiness position {} does not match query completion position {position}",
                ready.position()
            )));
        }
        Ok(Some(ready))
    }

    /// Drains remaining completions and returns the complete lowering product.
    pub fn finish(mut self) -> nia_query::QueryResult<BackendLowering> {
        while self.wait_next()?.is_some() {}
        Ok(self.collector.finish()?)
    }
}

/// Root-selection scope for backend code generation.
///
/// Runtime startup controls executable semantics, while package publication
/// selects every concrete runtime definition owned by the current package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CodegenScope {
    /// Emit definitions reachable from selected entry/runtime roots.
    #[default]
    Entry,
    /// Emit every non-generic runtime definition owned by the current package.
    Package,
}

/// User-visible result of checking a program.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgram {
    /// Loaded module graph used by the check.
    pub graph: ModuleGraphSnapshot,
    /// Optimization policy selected for subsequent lowering.
    pub optimization: OptimizationPolicy,
    /// Diagnostics emitted during checking.
    pub diagnostics: Vec<ProgramDiagnostic>,
    /// Number of downstream recovery diagnostics suppressed by phase gating.
    pub suppressed_downstream: usize,
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
    /// Number of downstream recovery diagnostics suppressed by phase gating.
    pub suppressed_downstream: usize,
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
            suppressed_downstream: self.suppressed_downstream,
            checked_body_count,
            reachable_body_count: checked_body_count,
        }
    }
}

/// Checked semantic products needed before backend lowering begins.
#[derive(Debug, Clone, PartialEq)]
pub struct CodegenPreparation {
    /// Canonical type store used by all checked modules.
    pub type_store: std::sync::Arc<nia_ty::TypeStore>,
    /// Module graph used by the preparation.
    pub graph: ModuleGraphSnapshot,
    /// Optimization policy selected for lowering.
    pub optimization: OptimizationPolicy,
    /// Checked semantic modules.
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    /// Collected monomorphization facts.
    pub monomorphization: std::sync::Arc<Monomorphization>,
    /// Diagnostics accumulated before code generation.
    pub diagnostics: Vec<ProgramDiagnostic>,
    /// Number of downstream recovery diagnostics suppressed by phase gating.
    pub suppressed_downstream: usize,
}

/// Complete checked and backend-lowered compiler product.
#[derive(Debug, Clone, PartialEq)]
pub struct CodegenProgram {
    /// Canonical type store used by the generated program.
    pub type_store: std::sync::Arc<nia_ty::TypeStore>,
    /// Module graph used by code generation.
    pub graph: ModuleGraphSnapshot,
    /// Optimization policy used by backend lowering.
    pub optimization: OptimizationPolicy,
    /// Checked semantic modules.
    pub modules: Vec<std::sync::Arc<CheckedModule>>,
    /// Collected monomorphization facts.
    pub monomorphization: std::sync::Arc<Monomorphization>,
    /// Backend-lowered module products.
    pub backend_lowering: std::sync::Arc<BackendLowering>,
    /// Diagnostics accumulated through code generation preparation.
    pub diagnostics: Vec<ProgramDiagnostic>,
    /// Number of downstream recovery diagnostics suppressed by phase gating.
    pub suppressed_downstream: usize,
}

/// Per-module checked products shared by later compiler queries.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedModule {
    /// Module identity in the checked graph.
    pub id: ModuleId,
    /// Canonical source path.
    pub path: SourcePath,
    /// Collected definitions.
    pub defs: std::sync::Arc<DefCollection>,
    /// Diagnostics emitted while collecting definitions.
    pub definition_diagnostics: nia_diagnostic::DiagnosticBundle,
    /// Resolved type facts.
    pub type_resolution: std::sync::Arc<TypeResolution>,
    /// Lowered type facts.
    pub type_lowering: std::sync::Arc<TypeLowering>,
    /// Resolved value facts.
    pub value_resolution: std::sync::Arc<ValueResolution>,
    /// Local binding and capture resolution.
    pub local_resolution: std::sync::Arc<LocalResolution>,
    /// Normalized projection and type facts.
    pub type_normalization: std::sync::Arc<TypeNormalization>,
    /// Checked constant expressions.
    pub const_eval: std::sync::Arc<ConstCheck>,
    /// Checked static initializers.
    pub static_check: std::sync::Arc<StaticCheck>,
    /// Computed target layouts.
    pub layouts: std::sync::Arc<Layouts>,
    /// ABI validation results.
    pub abi_check: std::sync::Arc<AbiCheck>,
    /// Control-flow validation results.
    pub flow_check: std::sync::Arc<FlowCheck>,
    /// Lowered body IR.
    pub body_ir: std::sync::Arc<BodyIr>,
    /// Semantic-use index for downstream consumers.
    pub semantic_uses: std::sync::Arc<SemanticUseTable>,
    /// Executable semantic facts.
    pub semantic_facts: std::sync::Arc<SemanticFacts>,
    /// Provider demands discovered by this module.
    pub provider_demands: std::sync::Arc<std::collections::HashSet<ProviderDemand>>,
    /// Reachable global definitions for executable generation.
    pub executable_reachable_globals: Option<std::collections::HashSet<GlobalDefId>>,
    /// Reachable nominal structs for executable generation.
    pub executable_reachable_structs:
        Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    /// Reachable nominal unions for executable generation.
    pub executable_reachable_unions: Option<std::sync::Arc<std::collections::HashSet<GlobalDefId>>>,
    /// Whether this module contributed type-only executable facts.
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
