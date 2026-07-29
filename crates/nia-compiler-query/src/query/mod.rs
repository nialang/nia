// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ActiveModuleItemTreeFactKind, CheckedModule, CheckedProgram, CheckedProgramAnalysis,
    CodegenPreparation, CodegenProgram, FrontendCheckInputFingerprint, FrontendCheckScope,
    ProgramDiagnostic, RuntimeModel, TimingMode, module_diagnostics,
};
#[cfg(test)]
use crate::{LoadedModule, LoadedProgram};
use nia_backend_lower::BackendLowerModuleInput;
use nia_const_check::{ConstCheck, ConstModuleLowering};
use nia_defs::{
    DefCollection, ModulePublicSurface, ModuleUsingScope, PublicSurfaceLookup,
    PublicSurfaceModuleFacts, PublicSurfaces, UsingScopeLookup,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
#[cfg(test)]
use nia_imports::ModuleGraph;
use nia_imports::{ModuleGraphLookup, ModuleGraphSnapshot, StableModuleKey};
use nia_item_signatures::{
    ItemSignatures, ProgramConstSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramStructSignature, ProgramTraitSignature,
    ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_node_id::NodeOriginTable;
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_parser::ParseError;
use nia_program_signatures::{
    ExtensionMethodIndexModuleInput, ExtensionMethodValidationInput, ExtensionModuleInput,
    ExtensionTraitSignatureIndex, ModuleProgramSignatureFacts, ModuleSignatureInput,
    VisibleExtensionsForModule, VisibleExtensionsInput, VisibleTraitImplsForModule,
    VisibleTypeSignatures, collect_extension_associated_value_index_for_module,
    collect_extension_method_diagnostics_for_module, collect_extension_method_index_for_module,
    collect_nominal_extension_providers_for_module, visible_extensions_for_module,
    visible_trait_impls_for_module,
};
use nia_public_surface::{
    TypeExposureIndex, compute_exported_public_surfaces_with_symbols,
    compute_using_scopes_from_surfaces_with_symbols,
};
use nia_query::{
    QueryDb, QueryError, QueryFingerprint, QueryFingerprintBuilder, QueryFingerprintPolicy,
    QueryFrame, QueryKey, QueryProviderPolicy, QueryResult, QueryStoragePolicy, QueryTrace,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_target_config::TargetConfig;
use nia_ty::{ArrayLenTy, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
};

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod context;
mod diagnostics;
mod executable;
mod extension_provider_queries;
mod function_body_queries;
mod program;
mod program_signature_queries;
mod providers;
mod registry;
mod resolve;
mod static_init_queries;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use context::*;
use diagnostics::*;
use executable::*;
use extension_provider_queries::*;
use function_body_queries::*;
use program::*;
use program_signature_queries::*;
use providers::*;
use registry::*;
use resolve::*;
use static_init_queries::*;
use types::*;

type ExtensionProviderModuleFactsValue = ExtensionProviderModuleFactsQueryValue;
type ExtensionProviderValidationFactsValue = ExtensionProviderValidationFactsQueryValue;
type ExtensionProviderNominalModuleFactsValue = ExtensionProviderNominalModuleFactsQueryValue;
type ExtensionProviderDiscoveryIndexValue = ExtensionProviderDiscoveryIndexQueryValue;
type ExtensionProviderNominalCandidateModulesValue =
    ExtensionProviderNominalCandidateModulesQueryValue;
type ExtensionProviderNominalModulesForTargetsValue =
    ExtensionProviderNominalModulesForTargetsQueryValue;
type TypeExposureIndexValue = TypeExposureIndex;
type ExtensionMethodIndexValue = ExtensionMethodIndexQueryValue;
type ExtensionMethodsNamedValue = ExtensionMethodsNamedQueryValue;
type ExtensionMethodByIdValue = ExtensionMethodByIdQueryValue;
type ExtensionTraitSignatureIndexValue = ExtensionTraitSignatureIndex;
type VisibleExtensionsValue = VisibleExtensionsForModule;
type VisibleTraitImplsValue = VisibleTraitImplsForModule;
type ExtensionSignatureModuleInputValue = ExtensionSignatureModuleInputQueryValue;
type ExtensionTraitSolvingModuleFactsValue = ExtensionTraitSolvingModuleFactsQueryValue;
type ExtensionTraitImplsForTraitValue = ExtensionTraitImplsForTraitQueryValue;
type ModuleProgramSignatureFactsValue = ModuleProgramSignatureFacts;
type ModuleAbiSignatureFactsValue = ModuleAbiSignatureFactsQueryValue;
type PublicSurfacesValue = PublicSurfacesQueryValue;
type PublicUsingScopesValue = PublicUsingScopesQueryValue;

#[derive(Clone)]
pub struct CompileRequest {
    loader_facts: Arc<dyn crate::LoaderFactProvider>,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
    frontend_cache_dir: Option<PathBuf>,
    verify_frontend_cache: bool,
}

impl CompileRequest {
    pub fn new(loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loader_facts);
        Self {
            loader_facts,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            frontend_cache_dir: None,
            verify_frontend_cache: false,
        }
    }

    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    pub fn with_frontend_cache_dir(mut self, frontend_cache_dir: Option<PathBuf>) -> Self {
        self.frontend_cache_dir = frontend_cache_dir;
        self
    }

    pub fn with_frontend_cache_verification(mut self, verify: bool) -> Self {
        self.verify_frontend_cache = verify;
        self
    }

    #[cfg(test)]
    fn with_loader_facts(mut self, loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        self.loader_facts = Arc::new(loader_facts);
        self
    }
}

#[derive(Clone)]
pub struct CompilerDatabase {
    db: QueryDb<CompilerContext>,
    inputs: Arc<RwLock<CompilerInputs>>,
}

impl CompilerDatabase {
    pub fn new(request: CompileRequest) -> Self {
        compiler_database_with_providers(request, CompilerQueryProviders::default())
    }

    pub fn query_session(&self) -> nia_query::QuerySession {
        self.db.session()
    }

    pub fn check_program(&self) -> QueryResult<CheckedProgram> {
        self.check_report(FrontendCheckScope::AllModules)
    }

    #[doc(hidden)]
    pub fn analyze_program(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.settle_provider_worklist(false, Self::check_program_once, checked_provider_demands)
    }

    fn check_program_once(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.db.get(CheckedProgramQuery).map(Arc::unwrap_or_clone)
    }

    pub fn entry_check_program(&self) -> QueryResult<CheckedProgram> {
        self.check_report(FrontendCheckScope::Entry)
    }

    #[doc(hidden)]
    pub fn analyze_entry_program(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.settle_provider_worklist(
            true,
            Self::entry_check_program_once,
            checked_provider_demands,
        )
    }

    fn entry_check_program_once(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.db
            .get(EntryCheckedProgramQuery)
            .map(Arc::unwrap_or_clone)
    }

    fn check_report(&self, scope: FrontendCheckScope) -> QueryResult<CheckedProgram> {
        let certificate_context = self.check_certificate_context(scope)?;
        let cached = certificate_context.and_then(|context| {
            let cache = self.db.context().signature_cache.as_ref()?;
            let lookup = cache.load_check_certificate(context.identity()).ok()?;
            Some((context, lookup))
        });
        if !self.db.context().verify_frontend_cache
            && let Some((_, crate::signature_cache::CheckCertificateLookup::Hit(certificate))) =
                cached.as_ref()
        {
            emit_check_certificate_reuse(self.db.context().timings(), true);
            self.db.context().loader_facts().settle_provider_demands()?;
            self.db
                .context()
                .provider_demand_rounds
                .store(0, std::sync::atomic::Ordering::Relaxed);
            return Ok(CheckedProgram {
                graph: self.current_graph()?,
                optimization: self.current_optimization(),
                diagnostics: certificate.diagnostics.clone(),
                checked_body_count: certificate.checked_body_count,
                reachable_body_count: certificate.reachable_body_count,
            });
        }
        emit_check_certificate_reuse(self.db.context().timings(), false);
        let report = match scope {
            FrontendCheckScope::AllModules => self.analyze_program()?.into_report(),
            FrontendCheckScope::Entry => self.analyze_entry_program()?.into_report(),
        };
        let Some(cache) = self.db.context().signature_cache.as_ref() else {
            return Ok(report);
        };
        let context = self.check_certificate_context(scope)?;
        if self.db.context().verify_frontend_cache
            && let Some((cached_context, crate::signature_cache::CheckCertificateLookup::Hit(_))) =
                cached.as_ref()
            && context
                .as_ref()
                .is_none_or(|context| context.key() != cached_context.key())
        {
            cache.remove_check_certificate(cached_context.key());
        }
        let Some(context) = context else {
            return Ok(report);
        };
        let certificate = crate::signature_cache::CachedCheckCertificate {
            checked_body_count: report.checked_body_count(),
            reachable_body_count: report.reachable_body_count(),
            diagnostics: report.diagnostics.clone(),
        };
        let _ = cache.publish_check_certificate(
            context.identity(),
            certificate,
            self.db.context().verify_frontend_cache,
        );
        Ok(report)
    }

    fn check_certificate_context(
        &self,
        scope: FrontendCheckScope,
    ) -> QueryResult<Option<CheckCertificateContext>> {
        let program_sources = self.db.get(FrontendProgramSourcesQuery)?;
        let Some(program_sources) = program_sources.as_ref().as_ref() else {
            return Ok(None);
        };
        let graph = self.db.get(ModuleGraphQuery)?;
        let Some(entry) = graph.stable_key(graph.entry()).cloned() else {
            return Ok(None);
        };
        let provider_facts = self.db.context().provider_fact_worklist()?;
        let input = check_certificate_input_fingerprint(
            program_sources.fingerprint,
            &graph,
            &provider_facts,
        );
        Ok(Some(CheckCertificateContext {
            namespace: crate::FrontendCacheNamespace::new(
                &self.db.context().loader_facts().target(),
                self.db.context().loader_facts().runtime(),
            ),
            entry,
            input,
            scope,
            source_lengths: program_sources
                .by_module
                .values()
                .map(|source| {
                    (
                        source.module.source_identity().normalized_path().to_owned(),
                        source.len,
                    )
                })
                .collect(),
        }))
    }

    fn executable_provider_demands(&self) -> QueryResult<Vec<crate::ProviderDemand>> {
        self.db
            .get(ExecutableProviderDemandsQuery)
            .map(Arc::unwrap_or_clone)
    }

    pub fn provider_fact_revision(&self) -> QueryResult<crate::ProviderFactRevision> {
        self.db
            .get(ProviderFactRevisionQuery)
            .map(|revision| *revision)
    }

    pub fn codegen_program(&self) -> QueryResult<CodegenProgram> {
        self.settle_provider_worklist(true, Self::codegen_program_once, codegen_provider_demands)
    }

    pub fn codegen_preparation(&self) -> QueryResult<CodegenPreparation> {
        self.settle_provider_worklist(
            true,
            Self::codegen_preparation_once,
            codegen_preparation_provider_demands,
        )
    }

    pub fn with_backend_finalization_schedule<R>(
        &self,
        consume: impl for<'borrow, 'stream, 'executor> FnOnce(
            Result<
                crate::BackendFinalizationSchedule<'borrow, 'stream, 'executor>,
                nia_backend_lower::BackendLowering,
            >,
        ) -> R,
    ) -> QueryResult<R> {
        providers::with_backend_finalization_schedule(&self.db, consume)
    }

    fn codegen_preparation_once(&self) -> QueryResult<CodegenPreparation> {
        self.db
            .get(CodegenPreparationQuery)
            .map(Arc::unwrap_or_clone)
    }

    fn codegen_program_once(&self) -> QueryResult<CodegenProgram> {
        self.db.get(CodegenProgramQuery).map(Arc::unwrap_or_clone)
    }

    fn settle_provider_worklist<T>(
        &self,
        discover_executable_providers: bool,
        compile: impl Fn(&Self) -> QueryResult<T>,
        provider_demands: impl Fn(&T) -> Vec<crate::ProviderDemand>,
    ) -> QueryResult<T> {
        let mut skip_executable_discovery = false;
        let mut rounds = 0_u64;
        loop {
            rounds += 1;
            if discover_executable_providers && !skip_executable_discovery {
                let demands = self.executable_provider_demands()?;
                emit_provider_demand_batch(self.db.context().timings(), rounds, &demands);
                if let crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } = self
                    .db
                    .context()
                    .loader_facts()
                    .update_provider_demands(demands)?
                {
                    nia_timing::emit_counter(
                        format!(
                            "compiler.executable_provider_demands.round_{rounds}.graph_changed"
                        ),
                        1,
                    );
                    nia_timing::emit_counter(
                        format!(
                            "compiler.executable_provider_demands.round_{rounds}.invalidates_body_facts"
                        ),
                        u64::from(invalidates_resolved_body_facts),
                    );
                    skip_executable_discovery = !invalidates_resolved_body_facts;
                    continue;
                }
            }
            let output = compile(self)?;
            match self
                .db
                .context()
                .loader_facts()
                .update_provider_demands(provider_demands(&output))?
            {
                crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } => {
                    skip_executable_discovery =
                        discover_executable_providers && !invalidates_resolved_body_facts;
                }
                crate::ProviderGraphUpdate::Stable => {
                    self.db.context().loader_facts().settle_provider_demands()?;
                    self.db
                        .context()
                        .provider_demand_rounds
                        .store(rounds, std::sync::atomic::Ordering::Relaxed);
                    return Ok(output);
                }
            }
        }
    }

    pub fn provider_demand_rounds(&self) -> u64 {
        self.db
            .context()
            .provider_demand_rounds
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn update(&self, request: CompileRequest) -> QueryResult<CompilerInvalidation> {
        let loader_session = request.loader_facts.query_session().unwrap_or_else(|| {
            panic!("Nia ICE: compiler updates require a tracked loader fact provider")
        });
        assert!(
            self.db.session().ptr_eq(&loader_session),
            "Nia ICE: compiler update loader facts belong to a different query session"
        );
        assert_eq!(
            request.frontend_cache_dir.as_deref(),
            self.db
                .context()
                .signature_cache
                .as_ref()
                .map(|cache| cache.root()),
            "Nia ICE: compiler frontend cache root cannot change within a query session"
        );
        assert_eq!(
            request.verify_frontend_cache,
            self.db.context().verify_frontend_cache,
            "Nia ICE: compiler frontend cache verification cannot change within a query session"
        );
        let new_inputs = CompilerInputs::new(request);
        let optimization_changed = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            *inputs = new_inputs;
            optimization_changed
        };
        self.invalidate_inputs(optimization_changed)
    }

    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }

    fn current_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        self.db.context().loader_facts.module_graph()
    }

    fn current_optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn invalidate_inputs(&self, optimization_changed: bool) -> QueryResult<CompilerInvalidation> {
        let mut invalidation = CompilerInvalidation::default();
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist),
        );
        if optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        Ok(invalidation)
    }
}

fn emit_provider_demand_batch(timings: TimingMode, round: u64, demands: &[crate::ProviderDemand]) {
    if !timings.enabled() {
        return;
    }
    let mut methods = 0_u64;
    let mut trait_impls = 0_u64;
    let mut module_semantics = 0_u64;
    let mut module_bodies = 0_u64;
    for demand in demands {
        match demand.request {
            crate::ProviderRequest::Method { .. } => methods += 1,
            crate::ProviderRequest::TraitImpl { .. } => trait_impls += 1,
            crate::ProviderRequest::ModuleSemantic { .. } => module_semantics += 1,
            crate::ProviderRequest::ModuleBody { .. } => module_bodies += 1,
        }
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}");
    nia_timing::emit_counter(format!("{prefix}.total"), demands.len() as u64);
    nia_timing::emit_counter(format!("{prefix}.methods"), methods);
    nia_timing::emit_counter(format!("{prefix}.trait_impls"), trait_impls);
    nia_timing::emit_counter(format!("{prefix}.module_semantics"), module_semantics);
    nia_timing::emit_counter(format!("{prefix}.module_bodies"), module_bodies);
}

fn emit_check_certificate_reuse(timings: TimingMode, hit: bool) {
    if !timings.enabled() {
        return;
    }
    nia_timing::emit_counter("compiler.check_certificate_hits", u64::from(hit));
    nia_timing::emit_counter("compiler.check_certificate_misses", u64::from(!hit));
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        f.debug_struct("CompilerDatabase")
            .field("optimization", &inputs.optimization)
            .finish_non_exhaustive()
    }
}

fn checked_provider_demands(program: &CheckedProgramAnalysis) -> Vec<crate::ProviderDemand> {
    program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

fn codegen_provider_demands(program: &CodegenProgram) -> Vec<crate::ProviderDemand> {
    program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

fn codegen_preparation_provider_demands(
    preparation: &CodegenPreparation,
) -> Vec<crate::ProviderDemand> {
    preparation
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerInvalidation {
    pub invalidated: Vec<QueryFrame>,
}

impl CompilerInvalidation {
    fn extend(&mut self, invalidation: nia_query::QueryInvalidation) {
        for frame in invalidation.invalidated {
            if !self.invalidated.contains(&frame) {
                self.invalidated.push(frame);
            }
        }
    }
}

fn compiler_database_with_providers(
    request: CompileRequest,
    providers: CompilerQueryProviders,
) -> CompilerDatabase {
    let session = request.loader_facts.query_session().unwrap_or_default();
    compiler_database_with_providers_in_session(request, providers, session)
}

fn compiler_database_with_providers_in_session(
    request: CompileRequest,
    providers: CompilerQueryProviders,
    session: nia_query::QuerySession,
) -> CompilerDatabase {
    let timings = request.timings;
    let signature_cache = request.frontend_cache_dir.as_ref().map(|root| {
        Arc::new(crate::signature_cache::PersistentSignatureCache::new(
            root.clone(),
        ))
    });
    let verify_frontend_cache = request.verify_frontend_cache;
    let loader_facts = Arc::clone(&request.loader_facts);
    if let Some(loader_session) = loader_facts.query_session() {
        assert!(
            session.ptr_eq(&loader_session),
            "Nia ICE: compiler and loader facts must share one query session"
        );
    }
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let executable_fact_session = Arc::new(std::sync::Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new());
    let db = QueryDb::new_registered_with_timings_in_session(
        CompilerContext {
            inputs: inputs.clone(),
            loader_facts,
            providers,
            executable_fact_session,
            executable_fact_scheduler: std::sync::Mutex::new(()),
            type_store,
            diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
            node_store,
            signature_cache,
            verify_frontend_cache,
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        timings,
        compiler_query_registry(),
        session,
    );
    CompilerDatabase { db, inputs }
}

pub(crate) fn query_error_diagnostic(err: QueryError) -> Diagnostic {
    match err {
        QueryError::Cycle { cycle } => {
            let mut message = String::from("query cycle detected");
            for frame in cycle {
                message.push_str("\n  ");
                message.push_str(&frame.description);
            }
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query cycle has no source span")
                .finish()
        }
        QueryError::InvalidInput { query, message } => {
            let message = format!("invalid query input for {}: {message}", query.description);
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query input has no source span")
                .finish()
        }
    }
}

fn stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> QueryResult<StableModuleSequence> {
    db.context().stable_module_sequence(module_ids)
}

fn resolve_stable_module_sequence_from_current_inputs(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    db.context().resolve_stable_module_sequence(sequence)
}

fn resolve_stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    let _graph = db.get(ModuleGraphQuery)?;
    db.context().resolve_stable_module_sequence(sequence)
}

fn provider_fact_worklist_fingerprint(worklist: &crate::ProviderFactSnapshot) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.provider-fact-worklist.v1");
    builder.write_fingerprint(provider_fact_revision_fingerprint(worklist.revision()));
    builder.write_fingerprint(provider_fact_revision_fingerprint(
        worklist.reset_revision(),
    ));
    let mut changes = worklist
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    changes.sort_unstable();
    builder.write_u64(changes.len() as u64);
    for change in changes {
        builder.write_fingerprint(change);
    }
    builder.finish()
}

fn provider_fact_revision_fingerprint(revision: crate::ProviderFactRevision) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.provider-fact-revision.v1");
    for part in revision.fingerprint_parts() {
        builder.write_u64(part);
    }
    builder.finish()
}

fn provider_demand_fingerprint(demand: &crate::ProviderDemand) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.provider-demand.v1");
    builder.write_str(demand.source_path.as_str());
    match &demand.request {
        crate::ProviderRequest::Method {
            target_type_name,
            method_name,
        } => {
            builder.write_u8(0);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(method_name.raw());
        }
        crate::ProviderRequest::TraitImpl { trait_name } => {
            builder.write_u8(1);
            builder.write_u64(trait_name.raw());
        }
        crate::ProviderRequest::ModuleSemantic { module_path } => {
            builder.write_u8(2);
            builder.write_str(module_path.as_str());
        }
        crate::ProviderRequest::ModuleBody { module_path } => {
            builder.write_u8(3);
            builder.write_str(module_path.as_str());
        }
    }
    builder.finish()
}

fn check_certificate_input_fingerprint(
    program_sources: crate::FrontendProgramSourceFingerprint,
    graph: &nia_imports::ModuleGraphSnapshot,
    provider_facts: &crate::ProviderFactSnapshot,
) -> FrontendCheckInputFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.check-certificate-input.v1");
    builder.write_fingerprint(QueryFingerprint::from_parts(program_sources.parts()));
    let mut modules = graph.modules().collect::<Vec<_>>();
    modules.sort_unstable_by(|left, right| {
        left.stable_key
            .source_identity()
            .normalized_path()
            .cmp(right.stable_key.source_identity().normalized_path())
    });
    builder.write_u64(modules.len() as u64);
    for module in modules {
        builder.write_str(module.stable_key.source_identity().normalized_path());
        builder.write_u64(module.module_path.package.raw());
        builder.write_u64(module.module_path.segments.len() as u64);
        for segment in &module.module_path.segments {
            builder.write_u64(segment.raw());
        }
        if let Some(parent) = module.parent {
            builder.write_u8(1);
            builder.write_str(
                graph
                    .stable_key(parent)
                    .expect("module graph parent must have stable identity")
                    .source_identity()
                    .normalized_path(),
            );
        } else {
            builder.write_u8(0);
        }
        builder.write_u8(u8::from(graph.is_executable_root_module(module.id)));
        let mut declarations = module
            .declarations
            .iter()
            .map(|declaration| {
                (
                    declaration.name.raw(),
                    visibility_tag(declaration.visibility),
                    graph
                        .stable_key(declaration.target)
                        .expect("module declaration target must have stable identity")
                        .source_identity()
                        .normalized_path()
                        .to_owned(),
                )
            })
            .collect::<Vec<_>>();
        declarations.sort_unstable();
        builder.write_u64(declarations.len() as u64);
        for (name, visibility, target) in declarations {
            builder.write_u64(name);
            builder.write_u8(visibility);
            builder.write_str(&target);
        }
    }
    let mut demands = provider_facts
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    demands.sort_unstable();
    builder.write_u64(demands.len() as u64);
    for demand in demands {
        builder.write_fingerprint(demand);
    }
    FrontendCheckInputFingerprint::from_parts(builder.finish().parts())
}

fn visibility_tag(visibility: nia_ids::Visibility) -> u8 {
    match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    }
}

fn module_graph_path_fingerprint(path: &Option<nia_imports::ModulePath>) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.module-graph-path.v1");
    let Some(path) = path else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    builder.write_u64(path.package.raw());
    builder.write_u64(path.segments.len() as u64);
    for segment in &path.segments {
        builder.write_u64(segment.raw());
    }
    builder.finish()
}

fn stable_module_key_fingerprint(domain: &str, key: &StableModuleKey) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    write_stable_module_key(&mut builder, key);
    builder.finish()
}

fn optional_stable_module_key_fingerprint(
    domain: &str,
    key: Option<&StableModuleKey>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    if let Some(key) = key {
        builder.write_u8(1);
        write_stable_module_key(&mut builder, key);
    } else {
        builder.write_u8(0);
    }
    builder.finish()
}

fn module_graph_child_fingerprint(
    child: &Option<(StableModuleKey, nia_ids::Visibility)>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.module-graph-child.v1");
    let Some((key, visibility)) = child else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    write_stable_module_key(&mut builder, key);
    builder.write_u8(match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    });
    builder.finish()
}

fn write_stable_module_key(builder: &mut QueryFingerprintBuilder, key: &StableModuleKey) {
    builder.write_str(key.source_identity().normalized_path());
}

fn stable_module_sequence_fingerprint(
    domain: &str,
    sequence: &StableModuleSequence,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(sequence.keys.len() as u64);
    for key in &sequence.keys {
        write_stable_module_key(&mut builder, key);
    }
    builder.finish()
}

fn source_path_fingerprint(domain: &str, path: &SourcePath) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(path.as_str());
    builder.finish()
}

fn source_version_fingerprint(domain: &str, version: SourceVersion) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(u64::from(version.id.0));
    builder.write_u64(version.revision.0);
    builder.finish()
}

fn provider_summary_fingerprint(
    summary: &nia_provider_summary::ProviderSummary,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.compiler.provider-summary.v1");
    builder.write_u64(summary.providers().len() as u64);
    for provider in summary.providers() {
        provider_type_ref_fingerprint(&mut builder, &provider.target.ty);
        if let Some(trait_ref) = &provider.trait_ref {
            builder.write_u8(1);
            provider_type_ref_fingerprint(&mut builder, trait_ref);
        } else {
            builder.write_u8(0);
        }
        builder.write_u64(provider.associated_methods.len() as u64);
        for method in &provider.associated_methods {
            builder.write_u64(method.raw());
        }
        builder.write_u64(provider.associated_values.len() as u64);
        for value in &provider.associated_values {
            builder.write_u64(value.raw());
        }
    }
    builder.finish()
}

fn provider_type_ref_fingerprint(
    builder: &mut QueryFingerprintBuilder,
    type_ref: &nia_provider_summary::ProviderTypeRef,
) {
    if let Some(last_name) = type_ref.last_name {
        builder.write_u8(1);
        builder.write_u64(last_name.raw());
    } else {
        builder.write_u8(0);
    }
    builder.write_u8(u8::from(type_ref.is_generic_or_structural_target));
    builder.write_u8(u8::from(type_ref.semantic_is_conservative));
}

fn bool_query_fingerprint(domain: &str, value: bool) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u8(u8::from(value));
    builder.finish()
}

impl CompilerContext {
    fn loader_facts(&self) -> &dyn crate::LoaderFactProvider {
        self.loader_facts.as_ref()
    }

    fn type_store(&self) -> &nia_ty::TypeStore {
        &self.type_store
    }

    fn node_store(&self) -> &nia_node_id::NodeStore {
        &self.node_store
    }

    fn frontend_program_sources(
        &self,
        db: &QueryDb<CompilerContext>,
    ) -> QueryResult<Option<FrontendProgramSources>> {
        let modules = db.get(LoadedModulesQuery)?;
        let module_ids = resolve_stable_module_sequence_from_current_inputs(db, &modules)?;
        let mut by_module = HashMap::new();
        let mut module_by_path = HashMap::new();
        let mut path_by_module = HashMap::new();
        let mut fingerprint_inputs = Vec::new();
        for module_id in module_ids {
            let path = db.get(ModulePathQuery(module_id))?;
            let version = *db.get(ModuleSourceVersionQuery(module_id))?;
            let Some((source, len)) = self.loader_facts.module_source_fingerprint(module_id)?
            else {
                return Ok(None);
            };
            let module = StableModuleKey::from_source_identity(path.identity());
            let normalized_path = module.source_identity().normalized_path().to_string();
            if module_by_path
                .insert(normalized_path.clone(), module_id)
                .is_some()
                || path_by_module.insert(module_id, normalized_path).is_some()
            {
                return Ok(None);
            }
            fingerprint_inputs.push((module.clone(), source, len));
            by_module.insert(
                module_id,
                FrontendProgramSource {
                    module,
                    version,
                    len,
                },
            );
        }
        let fingerprint = crate::frontend_program_source_fingerprint(
            fingerprint_inputs
                .iter()
                .map(|(module, source, len)| (module, *source, *len)),
        );
        Ok(Some(FrontendProgramSources {
            fingerprint,
            by_module,
            module_by_path,
            path_by_module,
        }))
    }

    fn stable_module_sequence(
        &self,
        module_ids: impl IntoIterator<Item = ModuleId>,
    ) -> QueryResult<StableModuleSequence> {
        let graph = self.loader_facts.module_graph()?;
        let mut identities = Vec::new();
        for module_id in module_ids {
            graph
                .get(module_id)
                .unwrap_or_else(|| panic!("Nia ICE: module {module_id:?} is not loaded"));
            let path = self
                .loader_facts
                .module_path(module_id)?
                .unwrap_or_else(|| panic!("Nia ICE: module {module_id:?} has no source path"));
            identities.push(path.identity());
        }
        Ok(StableModuleSequence::from_source_identities(identities))
    }

    fn resolve_stable_module_sequence(
        &self,
        sequence: &StableModuleSequence,
    ) -> QueryResult<Vec<ModuleId>> {
        let graph = self.loader_facts.module_graph()?;
        let mut module_ids = Vec::with_capacity(sequence.keys.len());
        for key in &sequence.keys {
            let mut current = None;
            for module in graph.modules() {
                if self
                    .loader_facts
                    .module_path(module.id)?
                    .is_some_and(|path| path.identity() == *key.source_identity())
                {
                    current = Some(module.id);
                    break;
                }
            }
            module_ids.push(current.unwrap_or_else(|| {
                panic!(
                    "Nia ICE: stable loaded module `{}` is missing from current loader facts",
                    key.source_identity().normalized_path()
                )
            }));
        }
        Ok(module_ids)
    }

    fn module_path(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<SourcePath> {
        self.loader_facts().module_path(module_id)?.ok_or_else(|| {
            db.invalid_input(
                &ModulePathQuery(module_id),
                format!("missing loaded module {module_id:?}"),
            )
        })
    }

    fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<SourceVersion> {
        self.loader_facts()
            .module_source_version(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleSourceVersionQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<NodeOriginTable> {
        self.loader_facts()
            .module_origins(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleOriginsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_parse_errors(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<Vec<ParseError>> {
        self.loader_facts()
            .module_parse_errors(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleParseErrorsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn declaration_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &DeclarationModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn full_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &FullModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn declaration_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &DeclarationActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn full_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &FullActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn signature_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        set: nia_item_tree::SignatureItemSet,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Signature(set))?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureItemTreeQuery(module_id, set),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn signature_const_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::ConstSignature)?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureConstItemTreeQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_provider_summary(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<nia_provider_summary::ProviderSummary> {
        self.loader_facts()
            .module_provider_summary(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ExtensionProviderSummaryQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_id_for_stable_key(
        &self,
        stable_key: &StableModuleKey,
    ) -> QueryResult<Option<ModuleId>> {
        Ok(self
            .loader_facts
            .module_graph()?
            .module_id_for_stable_key(stable_key))
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.loader_facts().symbols()
    }

    fn provider_fact_worklist(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        self.loader_facts().provider_facts()
    }

    fn optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn timings(&self) -> TimingMode {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .timings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeModel;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceIdentity, SourceRevision};
    #[path = "backend_closure.rs"]
    mod backend_closure;
    #[path = "backend_orchestration.rs"]
    mod backend_orchestration;
    #[path = "backend_stage.rs"]
    mod backend_stage;
    #[path = "body_invalidation.rs"]
    mod body_invalidation;
    #[path = "checked_products.rs"]
    mod checked_products;
    #[path = "compiler_contracts.rs"]
    mod compiler_contracts;
    #[path = "compiler_incremental_consistency.rs"]
    mod compiler_incremental_consistency;
    #[path = "const_semantic_dependencies.rs"]
    mod const_semantic_dependencies;
    #[path = "database.rs"]
    mod database;
    #[path = "definition_invalidation.rs"]
    mod definition_invalidation;
    #[path = "executable_const.rs"]
    mod executable_const;
    #[path = "executable_const_metadata.rs"]
    mod executable_const_metadata;
    #[path = "executable_empty_body.rs"]
    mod executable_empty_body;
    #[path = "executable_entry_reachability.rs"]
    mod executable_entry_reachability;
    #[path = "executable_filtering.rs"]
    mod executable_filtering;
    #[path = "executable_generic_reachability.rs"]
    mod executable_generic_reachability;
    #[path = "executable_initializers.rs"]
    mod executable_initializers;
    #[path = "executable_type_only.rs"]
    mod executable_type_only;
    #[path = "executable_value_refs.rs"]
    mod executable_value_refs;
    #[path = "extension_dependencies.rs"]
    mod extension_dependencies;
    #[path = "extension_nominal_queries.rs"]
    mod extension_nominal_queries;
    #[path = "extension_provider_refresh.rs"]
    mod extension_provider_refresh;
    #[path = "fixture.rs"]
    mod fixture;
    #[path = "frontend_cache.rs"]
    mod frontend_cache;
    #[path = "frontend_invalidation.rs"]
    mod frontend_invalidation;
    #[path = "frontend_membership.rs"]
    mod frontend_membership;
    #[path = "frontend_products.rs"]
    mod frontend_products;
    #[path = "incremental_extension_body.rs"]
    mod incremental_extension_body;
    #[path = "incremental_static_initializers.rs"]
    mod incremental_static_initializers;
    #[path = "loader_facts.rs"]
    mod loader_facts;
    #[path = "persistent_cache.rs"]
    mod persistent_cache;
    #[path = "provider_incremental.rs"]
    mod provider_incremental;
    #[path = "query_handle_reuse.rs"]
    mod query_handle_reuse;
    #[path = "semantic_diagnostics.rs"]
    mod semantic_diagnostics;
    #[path = "semantic_query_dependencies.rs"]
    mod semantic_query_dependencies;
    #[path = "semantic_query_scope.rs"]
    mod semantic_query_scope;
    #[path = "signature_query_dependencies.rs"]
    mod signature_query_dependencies;
    #[path = "support.rs"]
    mod support;
    #[path = "type_store_session.rs"]
    mod type_store_session;
    #[path = "visible_extensions.rs"]
    mod visible_extensions;
    use database::CompilerDatabase;
    use fixture::*;
    use frontend_cache::*;
    use loader_facts::*;
    use support::*;

    struct VtableFunctionInstanceRef<'a> {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &'a [InternedTyId],
        const_args: &'a [nia_ty::ConstGenericArg],
    }

    fn backend_function_instance_matches_vtable_ref(
        type_store: &nia_ty::TypeStore,
        vtable: VtableFunctionInstanceRef<'_>,
        instance: &nia_backend_ir::BackendFunctionInstance,
    ) -> bool {
        let VtableFunctionInstanceRef {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = vtable;
        if instance.def_id != def_id || instance.arg_module_id != arg_module_id {
            return false;
        }
        if self_arg.is_some_and(|ty| type_store.get(ty).is_none())
            || args.iter().any(|ty| type_store.get(*ty).is_none())
            || const_args
                .iter()
                .any(|arg| type_store.get(arg.ty).is_none())
        {
            return false;
        }
        self_arg == instance.self_arg && args == instance.args && const_args == instance.const_args
    }
}
