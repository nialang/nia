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
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
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

    #[test]
    fn compiler_query_registry_covers_all_declared_query_contracts() {
        let descriptors = compiler_query_registry().descriptors();

        assert_eq!(descriptors.len(), 132);
        assert!(
            !descriptors
                .iter()
                .any(|descriptor| descriptor.name == "module_graph_node")
        );
        for name in [
            "body_activation_worklist",
            "executable_fact_epoch",
            "module_graph_entry",
            "module_graph_path",
            "module_graph_parent",
            "module_graph_child",
            "module_package_root",
            "provider_fact_revision",
            "provider_fact_worklist",
        ] {
            assert!(
                descriptors.iter().any(|descriptor| descriptor.name == name),
                "missing precise graph fact query `{name}`"
            );
        }
        assert!(
            descriptors
                .windows(2)
                .all(|pair| pair[0].name < pair[1].name)
        );
        assert!(descriptors.iter().all(|descriptor| {
            let expected_storage = if matches!(
                descriptor.name,
                "backend_item_plan" | "backend_module_item_plan" | "backend_module_finalization"
            ) {
                nia_query::QueryStoragePolicy::SingleConsumerOwned
            } else {
                nia_query::QueryStoragePolicy::CacheOwnedArc
            };
            let expected_provider = if descriptor.name == "backend_module_item_plan" {
                nia_query::QueryProviderPolicy::ExternallyPublished
            } else {
                nia_query::QueryProviderPolicy::KeyExecute
            };
            descriptor.context_type == std::any::type_name::<CompilerContext>()
                && descriptor.provider == expected_provider
                && descriptor.storage == expected_storage
        }));
        for descriptor in descriptors {
            let expected = match descriptor.name {
                "extension_provider_module_ids"
                | "extension_provider_module_eligibility"
                | "extension_provider_summary"
                | "loaded_modules"
                | "module_graph_child"
                | "module_graph_entry"
                | "module_graph_parent"
                | "module_graph_path"
                | "module_package_root"
                | "module_path"
                | "module_source_version"
                | "parse_ok_module_ids"
                | "program_signature_module_eligibility"
                | "program_signature_module_ids"
                | "provider_fact_revision"
                | "provider_fact_worklist"
                | "public_surface_module"
                | "semantic_module_ids"
                | "using_scope_module" => nia_query::QueryFingerprintPolicy::StableValue,
                "active_module_item_tree_input"
                | "backend_module_function_instance_plan"
                | "backend_module_source_item_plan"
                | "body_activation_worklist"
                | "declaration_active_module_item_tree_input"
                | "declaration_module_item_tree_input"
                | "executable_function_body"
                | "executable_static_init"
                | "full_active_module_item_tree_input"
                | "executable_fact_epoch"
                | "full_module_item_tree_input"
                | "lowered_function_body"
                | "module_public_surface"
                | "module_item_tree_input"
                | "module_origins"
                | "module_parse_errors"
                | "module_using_scope"
                | "public_surface_module_facts"
                | "public_surface_type"
                | "public_surface_value"
                | "public_surfaces"
                | "public_using_scopes"
                | "signature_const_item_tree"
                | "signature_item_tree"
                | "using_scope_type"
                | "using_scope_unresolved"
                | "using_scope_value" => nia_query::QueryFingerprintPolicy::SemanticValue,
                _ => nia_query::QueryFingerprintPolicy::None,
            };
            assert_eq!(descriptor.fingerprint, expected, "{}", descriptor.name);
        }
    }

    #[test]
    fn public_options_flow_through_compiler_query_context() {
        for level in [
            NiaOptimizationLevel::O0,
            NiaOptimizationLevel::O1,
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
            NiaOptimizationLevel::Os,
            NiaOptimizationLevel::Oz,
        ] {
            let fixture = LoadedProgramFixture::new(
                "main.nia",
                r#"
static zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#,
            );
            let checked = CompilerDatabase::new(
                CompileRequest::new(fixture.program()).with_optimization(level),
            )
            .codegen_program();
            let policy = level.policy();

            assert!(
                checked.diagnostics.is_empty(),
                "{level:?}: {:?}",
                checked.diagnostics
            );
            assert_eq!(checked.optimization, policy, "{level:?}");
            assert_eq!(checked.backend_lowering.optimization, policy, "{level:?}");
            assert_eq!(
                checked
                    .backend_lowering
                    .optimization_report
                    .enabled_global_passes,
                if policy.prefer_size
                    || policy.const_fold.at_least(nia_opt::OptimizationDepth::Full)
                {
                    vec!["simplify-static-init"]
                } else {
                    Vec::new()
                },
                "{level:?}"
            );
        }
    }

    #[test]
    fn compiler_database_exposes_query_trace() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let checked = database.check_program();
        let trace = database.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_program" && dependency.to.name == "checked_module_ids"
        }));
    }

    #[test]
    fn compiler_update_rejects_untracked_snapshot_provider() {
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = super::CompilerDatabase::new(CompileRequest::new(fixture.program()));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = database.update(CompileRequest::new(fixture.program()));
        }));

        assert!(result.is_err());
    }

    #[test]
    fn compiler_query_providers_can_override_query_execution() {
        fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> QueryResult<StableModuleSequence> {
            Ok(StableModuleSequence::default())
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: no_parse_ok_modules,
            ..CompilerQueryProviders::default()
        };
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let checked =
            compiler_database_with_providers(CompileRequest::new(fixture.program()), providers)
                .codegen_program()
                .expect("overridden codegen program");

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_propagates_query_failure() {
        fn unknown_module_id() -> ModuleId {
            let mut module_ids = nia_ids::ModuleIdAllocator::new();
            module_ids.allocate();
            module_ids.allocate()
        }

        fn unknown_checked_module(_: &QueryDb<CompilerContext>) -> QueryResult<Vec<ModuleId>> {
            Ok(vec![unknown_module_id()])
        }

        let providers = CompilerQueryProviders {
            checked_module_ids: unknown_checked_module,
            ..CompilerQueryProviders::default()
        };
        let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
        let database = compiler_database_with_providers(
            CompileRequest::new(fixture.program()).with_optimization(NiaOptimizationLevel::Oz),
            providers,
        );
        let missing_module = unknown_module_id();
        for error in [
            database
                .db
                .get(ModulePathQuery(missing_module))
                .expect_err("missing module path should be a query error"),
            database
                .db
                .get(ModuleItemTreeQuery(missing_module))
                .expect_err("missing module item tree should propagate its input query error"),
            database
                .db
                .get(ModuleDefsQuery(missing_module))
                .expect_err("module definitions should propagate a missing item tree error"),
            database
                .db
                .get(TypeResolutionQuery(missing_module))
                .expect_err("type resolution should propagate a missing module input"),
            database
                .db
                .get(TypeNormalizationQuery(missing_module))
                .expect_err("type normalization should propagate a missing module input"),
            database
                .db
                .get(SemanticUseTableQuery(missing_module))
                .expect_err("semantic uses should propagate a missing module input"),
            database
                .db
                .get(ConstModuleQuery(missing_module))
                .expect_err("const lowering should propagate a missing module input"),
            database
                .db
                .get(SignatureConstModuleQuery(missing_module))
                .expect_err("signature const lowering should propagate a missing module input"),
            database
                .db
                .get(ConstArrayLengthsQuery(missing_module))
                .expect_err("const array lengths should propagate a missing module input"),
            database
                .db
                .get(ConstEnumValuesQuery(missing_module))
                .expect_err("const enum values should propagate a missing module input"),
            database
                .db
                .get(ConstValuesQuery(missing_module))
                .expect_err("const values should propagate a missing module input"),
            database
                .db
                .get(ConstTypedFactsQuery(missing_module))
                .expect_err("const typed facts should propagate a missing module input"),
            database
                .db
                .get(ConstQuery(missing_module))
                .expect_err("const checking should propagate a missing module input"),
            database
                .db
                .get(SignatureLayoutsQuery(missing_module))
                .expect_err("signature layouts should propagate a missing module input"),
            database
                .db
                .get(LayoutsQuery(missing_module))
                .expect_err("layouts should propagate a missing module input"),
            database
                .db
                .get(StaticCheckQuery(missing_module))
                .expect_err("static checking should propagate a missing module input"),
            database
                .db
                .get(BodyCheckQuery(missing_module))
                .expect_err("body checking should propagate a missing module input"),
            full_body_check_resolution_inputs(&database.db, missing_module)
                .err()
                .expect("body resolution inputs should propagate a missing module input"),
            database
                .db
                .get(CheckedModuleQuery(missing_module))
                .expect_err("checked modules should propagate a missing module input"),
            database
                .db
                .get(CheckedProgramQuery)
                .expect_err("checked program aggregation should propagate a missing module input"),
            database
                .db
                .get(FlowCheckQuery(missing_module))
                .expect_err("flow checking should propagate a missing module input"),
            database
                .db
                .get(ModuleAbiSignatureFactsQuery(missing_module))
                .expect_err("ABI signature facts should propagate a missing module input"),
            database
                .db
                .get(AbiCheckQuery(missing_module))
                .expect_err("ABI checking should propagate a missing module input"),
            database
                .db
                .get(ExtensionSignatureModuleInputQuery(missing_module))
                .expect_err("extension signature input should propagate a missing module input"),
            database
                .db
                .get(ExtensionTraitSolvingModuleFactsQuery(missing_module))
                .expect_err("extension trait facts should propagate a missing module input"),
            database
                .db
                .get(ExtensionProviderValidationFactsQuery(missing_module))
                .expect_err("extension validation should propagate a missing module input"),
            database
                .db
                .get(VisibleExtensionsQuery(missing_module))
                .expect_err("visible extensions should propagate a missing module input"),
            database
                .db
                .get(ExecutableValueRefEdgesQuery(GlobalDefId {
                    module_id: missing_module,
                    def_id: nia_ids::DefId(0),
                }))
                .expect_err("value-ref edges should propagate a missing module input"),
        ] {
            assert!(matches!(error, QueryError::InvalidInput { .. }));
            assert!(
                error
                    .to_string()
                    .contains(&format!("missing loaded module {missing_module:?}"))
            );
        }

        let error = database
            .analyze_program()
            .expect_err("public analysis must propagate a missing module query failure");
        assert!(matches!(error, QueryError::InvalidInput { .. }));
        assert!(
            error
                .to_string()
                .contains(&format!("missing loaded module {:?}", unknown_module_id()))
        );
    }

    #[test]
    fn body_check_resolves_program_signatures_through_precise_signature_queries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "using helper::{Alias, value}; fn main() Alias { value() }",
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "helper",
            "helper.nia",
            "pub type Alias = i32; pub fn value() Alias { 1 }",
        );
        let db = query_db(fixture.program());

        let checked = db.expect_get(BodyCheckQuery(entry_id));
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let trace = db.query_trace();

        assert!(trace_has_dependency(
            &trace,
            "body_check",
            "signature_item_signatures"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "body_check",
            "signature_type_lowering"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "body_check",
            "visible_extensions"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "body_check",
            "program_trait_solving_signatures"
        ));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && matches!(
                    dependency.to.name,
                    "program_body_value_signatures"
                        | "program_body_type_signatures"
                        | "program_body_trait_signatures"
                )
        }));
    }

    #[test]
    fn body_check_resolves_trait_method_candidates_through_program_trait_method_index() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
module traits;
using entry::traits::{Ops, Value};

fn main() i32 {
    let value = Value {};
    value.used()
}
"#,
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "traits",
            "traits.nia",
            r#"
pub trait Ops {
    fn used(self) i32;
}

pub struct Value {}

extend Value : Ops {
    fn used(self) i32 {
        1
    }
}
"#,
        );
        let db = query_db(fixture.program());

        let checked = db.expect_get(BodyCheckQuery(entry_id));
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let trace = db.query_trace();

        assert!(trace_has_dependency(
            &trace,
            "body_check",
            "program_trait_method_index"
        ));
        assert!(trace_has_dependency(
            &trace,
            "program_trait_method_index",
            "module_program_signature_facts"
        ));
        assert!(trace_has_dependency(
            &trace,
            "program_trait_method_index",
            "program_signature_module_ids"
        ));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "module_program_signature_facts"
                && dependency.to.description.contains("Traits")
        }));
    }

    #[test]
    fn program_signature_module_ids_use_set_specific_module_facts() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module module1; module module2; module module3; module module4; module module5; module module6;",
        );
        let entry_id = fixture.entry_id();
        let module1 = fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            "struct S { value: i32 }",
        );
        let module2 =
            fixture.add_child(entry_id, "module2", "module2.nia", "fn helper() i32 { 1 }");
        let module3 = fixture.add_child(
            entry_id,
            "module3",
            "module3.nia",
            "const WIDTH: usize = 4usize;",
        );
        let module4 = fixture.add_child(
            entry_id,
            "module4",
            "module4.nia",
            "trait Read { fn read(self) i32; }",
        );
        let module5 = fixture.add_child(
            entry_id,
            "module5",
            "module5.nia",
            "struct T {} extend T { pub fn make() T { {} } }",
        );
        let module6 = fixture.add_child(
            entry_id,
            "module6",
            "module6.nia",
            "struct U {} extend U { const WIDTH: usize = 4usize; }",
        );
        let db = query_db(fixture.program());

        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.expect_get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Functions
                ))
            )
            .expect("function signature module sequence")
            .as_slice(),
            &[module2, module4, module5]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.expect_get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Values
                ))
            )
            .expect("value signature module sequence")
            .as_slice(),
            &[module3, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.expect_get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Types
                ))
            )
            .expect("type signature module sequence")
            .as_slice(),
            &[module1, module5, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.expect_get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::Traits
                ))
            )
            .expect("trait signature module sequence")
            .as_slice(),
            &[module4, module5, module6]
        );
        assert_eq!(
            resolve_stable_module_sequence(
                &db,
                &db.expect_get(ProgramSignatureModuleIdsQuery(
                    nia_item_tree::SignatureItemSet::ExtensionFunctions
                ))
            )
            .expect("extension signature module sequence")
            .as_slice(),
            &[module4, module5]
        );

        let trace = db.query_trace();
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_signature_module_ids"
                && dependency.to.name == "program_signature_module_eligibility"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_signature_module_eligibility"
                && dependency.to.name == "signature_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_signature_module_eligibility"
                && matches!(
                    dependency.to.name,
                    "signature_type_lowering" | "signature_item_signatures" | "module_defs"
                )
        }));
    }

    #[test]
    fn extension_provider_module_ids_use_parse_ok_provider_summaries() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module module1; module module2; module module3; module module4; module module5;",
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            "struct S { value: i32 }",
        );
        fixture.add_child(entry_id, "module2", "module2.nia", "fn helper() i32 { 1 }");
        fixture.add_child(
            entry_id,
            "module3",
            "module3.nia",
            "const WIDTH: usize = 4usize;",
        );
        fixture.add_child(
            entry_id,
            "module4",
            "module4.nia",
            "trait Read { fn read(self) i32; }",
        );
        let module5 = fixture.add_child(
            entry_id,
            "module5",
            "module5.nia",
            "struct T {} extend T { pub fn make() T { {} } }",
        );
        let db = query_db(fixture.program());

        assert_eq!(
            resolve_stable_module_sequence(&db, &db.expect_get(ExtensionProviderModuleIdsQuery))
                .expect("extension provider module sequence")
                .as_slice(),
            &[module5]
        );
        let trace = db.query_trace();
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_module_ids",
            "parse_ok_module_ids"
        ));
        assert!(trace_has_dependency(
            &trace,
            "extension_provider_module_ids",
            "extension_provider_module_eligibility"
        ));
    }

    #[test]
    fn program_type_alias_signature_uses_precise_module_facts() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "struct S { value: i32 } type Alias = S; fn helper() i32 { 1 }",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let defs = db.expect_get(ModuleDefsQuery(module_id));
        let alias_id = defs.semantic.module_scope.types.get(&sym("Alias")).unwrap();
        let _ = db.expect_get(ProgramTypeAliasSignatureQuery(GlobalDefId {
            module_id,
            def_id: alias_id,
        }));
        let trace = db.query_trace();

        assert!(trace_has_dependency(
            &trace,
            "program_type_alias_signature",
            "module_program_signature_facts"
        ));
        assert!(!trace_has_dependency(
            &trace,
            "program_type_alias_signature",
            "program_signature_module_ids"
        ));
    }

    #[test]
    fn layout_uses_full_type_module_signatures_and_array_lengths_without_body_products() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "struct S { value: i32 } fn helper() i32 { 1 }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let layouts = db.expect_get(LayoutsQuery(module_id));
        let trace = db.query_trace();

        assert!(
            layouts.semantic.diagnostics.is_empty(),
            "{:?}",
            layouts.semantic.diagnostics
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "layout_type_normalization"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "const_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && matches!(
                    dependency.to.name,
                    "type_normalization" | "const" | "body_check"
                )
        }));
    }

    #[test]
    fn layout_uses_signature_layouts_for_cross_module_types() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module module1; using self::module1::S; struct Holder { value: S }",
        );
        let entry_id = fixture.entry_id();
        let module1 = fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            "pub struct S { value: i32 } fn helper() i32 { 1 }",
        );
        let db = query_db(fixture.program());

        let layouts = db.expect_get(LayoutsQuery(entry_id));
        let trace = db.query_trace();
        let entry_description = format!("{entry_id:?}");
        let module1_description = format!("{module1:?}");

        assert!(
            layouts.semantic.diagnostics.is_empty(),
            "{:?}",
            layouts.semantic.diagnostics
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && dependency.from.description.contains(&entry_description)
                && dependency.to.name == "signature_layouts"
                && dependency.to.description.contains(&module1_description)
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && dependency.from.description.contains(&entry_description)
                && dependency.to.name == "layouts"
                && dependency.to.description.contains(&module1_description)
        }));
    }

    #[test]
    fn signature_layout_reads_canonical_types_from_store() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module module1; using self::module1::Box; struct Holder { value: Box[u16] }",
        );
        let entry_id = fixture.entry_id();
        fixture.add_child(
            entry_id,
            "module1",
            "module1.nia",
            "pub struct Box[T] { value: [3]T }",
        );
        let db = query_db(fixture.program());
        let signature_types = nia_item_tree::SignatureItemSet::Types;

        let _ = db.expect_get(SignatureTypeNormalizationQuery(entry_id, signature_types));
        let _ = db.expect_get(SignatureItemSignaturesQuery(entry_id, signature_types));
        let layouts = db.expect_get(SignatureLayoutsQuery(entry_id));

        assert!(
            layouts.semantic.diagnostics.is_empty(),
            "{:?}",
            layouts.semantic.diagnostics
        );
        assert!(
            layouts
                .semantic
                .types
                .keys()
                .all(|ty| db.context().type_store.get(*ty).is_some())
        );
    }

    #[test]
    fn abi_check_uses_abi_signature_index_not_body_signatures() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "extern struct S { value: i32 } extern fn take(value: S) void;",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(AbiCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "abi_check" && dependency.to.name == "program_abi_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_abi_signatures"
                && dependency.to.name == "module_abi_signature_facts"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_abi_signature_facts"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "abi_check" && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            matches!(
                dependency.from.name,
                "program_abi_signatures" | "module_abi_signature_facts"
            ) && matches!(
                dependency.to.name,
                "item_signatures" | "type_normalization" | "signature_type_lowering"
            )
        }));
        assert!(!depends_on_body_signature_query(&trace, "abi_check"));
    }

    #[test]
    fn const_uses_precise_program_context_queries() {
        let fixture =
            LoadedProgramFixture::new("main.nia", "const VALUE = 1; fn main() i32 { VALUE }");
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(ConstQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const" && dependency.to.name == "const_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const" && dependency.to.name == "const_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const" && dependency.to.name == "const_enum_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const_array_lengths" && dependency.to.name == "const_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const_enum_values"
                && dependency.to.name == "const_array_lengths"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const" && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_const_modules")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_item_signatures")
        );
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!depends_on_body_signature_query(&trace, "const"));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const" && dependency.to.name == "full_module_defs"
        }));
    }

    #[test]
    fn monomorphization_avoids_removed_program_trait_signature_product() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 1 }");
        let db = query_db(fixture.program());

        let _ = db.expect_get(MonomorphizationQuery);
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "monomorphization"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!depends_on_body_signature_query(&trace, "monomorphization"));
    }

    #[test]
    fn executable_reachability_uses_lazy_signature_resolvers() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 1 }");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let _ = db.expect_get(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "program_executable_reachability_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_module_facts"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!depends_on_body_signature_query(
            &trace,
            "executable_checked_module_facts"
        ));
        assert!(trace_has_dependency(
            &trace,
            "executable_checked_modules",
            "executable_checked_module_facts"
        ));
    }

    #[test]
    fn body_check_without_method_lookup_does_not_build_global_extension_method_index() {
        let mut fixture =
            LoadedProgramFixture::new("main.nia", "module providers; fn main() i32 { 1 }");
        let module_id = fixture.entry_id();
        fixture.add_child(
            module_id,
            "providers",
            "providers.nia",
            "struct S {} extend S { pub fn make() S { {} } }",
        );
        let db = query_db(fixture.program());

        let checked = db.expect_get(BodyCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(
            !trace
                .queries
                .iter()
                .any(|query| { query.frame.name == "extension_method_index" })
        );
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "extension_method_index"
        }));
    }

    #[test]
    fn body_check_method_lookup_uses_named_extension_method_query() {
        let mut fixture = LoadedProgramFixture::new(
            "main.nia",
            "module module1; using self::module1::S; fn main() i32 { let s = S::make(); 1 }",
        );
        let module_id = fixture.entry_id();
        fixture.add_child(
            module_id,
            "module1",
            "module1.nia",
            "pub struct S {} extend S { pub fn make() S { {} } }",
        );
        let db = query_db(fixture.program());

        let checked = db.expect_get(BodyCheckQuery(module_id));
        let trace = db.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "extension_methods_named"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "extension_method_index"
        }));
    }

    #[test]
    fn const_module_uses_full_active_item_tree_query() {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "const fn value() usize { 1 } const VALUE = value();",
        );
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let _ = db.expect_get(ConstModuleQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "const_module"
                && dependency.to.name == "full_active_module_item_tree"
        }));
    }

    #[test]
    fn semantic_use_table_query_combines_value_local_and_type_resolution() {
        let source = "static VALUE: i32 = 1; fn main() i32 { let mut local: i32 = VALUE; local }";
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());

        let table = db.expect_get(SemanticUseTableQuery(module_id));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "value_resolution"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "local_resolution"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "type_lowering"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "module_origins"
        }));
        assert_eq!(table.store_id(), db.context().node_store().id());

        assert!(matches!(
            table
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Global(_))),
            Some(SemanticValueUse::Global(_))
        ));

        assert!(matches!(
            table
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
            Some(SemanticValueUse::Local(_))
        ));

        assert!(!table.node_type_uses.is_empty());
    }

    #[test]
    fn resolution_queries_share_compiler_session_node_owner() {
        let source = "static VALUE: i32 = 1; fn main() i32 { let local: i32 = VALUE; local }";
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let module_id = fixture.entry_id();
        let db = query_db(fixture.program());
        let node_store_id = db.context().node_store().id();

        let values = db.expect_get(ValueResolutionQuery(module_id));
        assert_eq!(values.semantic.node_names.store_id(), node_store_id);
        assert_eq!(
            values.semantic.node_qualified_values.store_id(),
            node_store_id
        );
        assert_eq!(
            values.semantic.node_builtin_associated_values.store_id(),
            node_store_id
        );
        assert_eq!(values.semantic.node_variant_enums.store_id(), node_store_id);
        assert_eq!(
            values.semantic.node_qualified_type_prefixes.store_id(),
            node_store_id
        );

        let locals = db.expect_get(LocalResolutionQuery(module_id));
        assert_eq!(locals.semantic.node_local_defs.store_id(), node_store_id);
        assert_eq!(locals.semantic.node_uses.store_id(), node_store_id);

        let types = db.expect_get(TypeResolutionQuery(module_id));
        assert_eq!(
            types.semantic.node_const_generic_names.store_id(),
            node_store_id
        );
    }

    #[test]
    fn executable_function_body_produces_factless_empty_body() {
        let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() void {}");
        let mut loaded = fixture.program();
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
        let module = facts.modules.first().expect("entry module facts");
        let def_id = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym("main")).then_some(GlobalDefId {
                    module_id: module.id,
                    def_id,
                })
            })
            .expect("main function definition");
        assert!(facts.runtime_functions.contains(&def_id));
        assert!(
            !module.semantic_facts.function_facts.contains_key(&def_id),
            "an empty body should not need a synthetic semantic-facts entry"
        );

        let body = db.expect_get(ExecutableFunctionBodyQuery(def_id));
        let body = body.as_ref().as_ref().expect("empty checked body product");
        assert!(body.stmts.is_empty());
        assert!(body.tail.is_none());

        let checked = db.expect_get(ExecutableCheckedModulesQuery);
        let aggregate_body = checked
            .iter()
            .find(|module| module.id == def_id.module_id)
            .and_then(|module| module.body_ir.function_bodies.get(&def_id))
            .expect("aggregate empty checked body");
        assert!(Arc::ptr_eq(body, aggregate_body));

        let codegen = db.expect_get(CodegenProgramQuery);
        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
        assert!(
            codegen
                .backend_lowering
                .program
                .modules
                .iter()
                .flat_map(|module| &module.functions)
                .any(|function| function.def_id == def_id)
        );
    }
}
